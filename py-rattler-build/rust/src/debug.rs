use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
    sync::{Arc, Mutex},
};

use ::rattler_build::{
    metadata::{BuildConfiguration, Output, PlatformWithVirtualPackages},
    render::resolved_dependencies::RunExportsDownload,
    source::patch::apply_patch_custom,
    system_tools::SystemTools,
    types::{BuildSummary, Directories, PackageIdentifier, PackagingSettings},
};
use pyo3::prelude::*;
use rattler_build_recipe::stage1::HashInfo;
use rattler_build_types::NormalizedKey;
use rattler_conda_types::{NamedChannelOrUrl, Platform};
use rattler_solve::SolveStrategy;

use crate::error::RattlerBuildError;
use crate::render;
use crate::run_async_task;
use crate::tool_config;
use crate::tracing_subscriber;

/// Result of running a build script in a debug session
#[pyclass(name = "DebugRunResult", from_py_object)]
#[derive(Clone)]
pub struct DebugRunResultPy {
    /// Exit code of the build script
    #[pyo3(get)]
    pub exit_code: i32,
    /// Captured stdout
    #[pyo3(get)]
    pub stdout: String,
    /// Captured stderr
    #[pyo3(get)]
    pub stderr: String,
}

#[pymethods]
impl DebugRunResultPy {
    fn __repr__(&self) -> String {
        format!(
            "DebugRunResult(exit_code={}, stdout={} bytes, stderr={} bytes)",
            self.exit_code,
            self.stdout.len(),
            self.stderr.len()
        )
    }
}

/// Interactive debug session for iterating on a recipe build
#[pyclass(name = "DebugSession", from_py_object)]
#[derive(Clone)]
pub struct DebugSessionPy {
    /// Work directory where sources are extracted
    #[pyo3(get)]
    pub work_dir: PathBuf,
    /// Host prefix directory
    #[pyo3(get)]
    pub host_prefix: PathBuf,
    /// Build prefix directory
    #[pyo3(get)]
    pub build_prefix: PathBuf,
    /// Build directory (parent of work_dir)
    #[pyo3(get)]
    pub build_dir: PathBuf,
    /// Path to the build script (conda_build.sh or conda_build.bat)
    #[pyo3(get)]
    pub build_script: PathBuf,
    /// Path to the recipe file
    #[pyo3(get)]
    pub recipe_path: PathBuf,
    /// Output directory for built packages
    #[pyo3(get)]
    pub output_dir: PathBuf,
    /// Captured log messages from setup
    #[pyo3(get)]
    pub log: Vec<String>,
}

#[pymethods]
impl DebugSessionPy {
    /// Set up a debug session from a rendered variant.
    ///
    /// This resolves dependencies, fetches sources, installs environments,
    /// and creates the build script — but does NOT run the build.
    #[staticmethod]
    #[pyo3(signature = (rendered_variant, channels=None, output_dir=None, tool_config=None, recipe_path=None, no_build_id=true, progress_callback=None))]
    #[allow(clippy::too_many_arguments)]
    fn setup(
        rendered_variant: render::PyRenderedVariant,
        channels: Option<Vec<String>>,
        output_dir: Option<PathBuf>,
        tool_config: Option<tool_config::PyToolConfiguration>,
        recipe_path: Option<PathBuf>,
        no_build_id: bool,
        progress_callback: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let tool_config = tool_config.map(|tc| tc.inner).unwrap_or_else(|| {
            ::rattler_build::tool_configuration::Configuration::builder().finish()
        });

        let channels: Vec<String> = channels.unwrap_or_else(|| vec!["conda-forge".to_string()]);

        let channels: Vec<NamedChannelOrUrl> = channels
            .iter()
            .map(|c| {
                NamedChannelOrUrl::from_str(c)
                    .map_err(|e| RattlerBuildError::Channel(e.to_string()))
            })
            .collect::<Result<_, _>>()?;

        // Build Output object (same pattern as build.rs)
        let timestamp = chrono::Utc::now();
        let virtual_package_override =
            rattler_virtual_packages::VirtualPackageOverrides::from_env();

        let mut subpackages = BTreeMap::new();
        let recipe = &rendered_variant.inner.recipe;
        subpackages.insert(
            recipe.package.name.clone(),
            PackageIdentifier {
                name: recipe.package.name.clone(),
                version: recipe.package.version.clone(),
                build_string: recipe
                    .build
                    .string
                    .as_resolved()
                    .ok_or_else(|| {
                        RattlerBuildError::Other("Build string not resolved".to_string())
                    })?
                    .to_string(),
            },
        );

        let recipe = rendered_variant.inner.recipe;
        let variant = rendered_variant.inner.variant;
        let hash_info = rendered_variant.inner.hash_info;

        let target_platform = variant
            .get(&NormalizedKey("target_platform".to_string()))
            .and_then(|v| v.to_string().parse::<Platform>().ok())
            .unwrap_or_else(Platform::current);

        let build_platform = variant
            .get(&NormalizedKey("build_platform".to_string()))
            .and_then(|v| v.to_string().parse::<Platform>().ok())
            .unwrap_or_else(Platform::current);

        let host_platform = variant
            .get(&NormalizedKey("host_platform".to_string()))
            .and_then(|v| v.to_string().parse::<Platform>().ok())
            .unwrap_or_else(Platform::current);

        let channels_urls = channels
            .iter()
            .map(|c| c.clone().into_base_url(&tool_config.channel_config))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RattlerBuildError::Other(format!("Channel error: {}", e)))?;

        let hash_info = hash_info.unwrap_or_else(|| HashInfo {
            hash: String::new(),
            prefix: String::new(),
        });

        let build_name = recipe.package.name.as_normalized().to_string();

        // Determine recipe_path and output_dir
        let safe_recipe_path = recipe_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("recipe.yaml"));

        let output_dir = output_dir.unwrap_or_else(|| {
            safe_recipe_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("output")
        });

        let output = Output {
            recipe,
            build_configuration: BuildConfiguration {
                target_platform,
                host_platform: PlatformWithVirtualPackages::detect_for_platform(
                    host_platform,
                    &virtual_package_override,
                )
                .map_err(|e| {
                    RattlerBuildError::Other(format!("Platform detection error: {}", e))
                })?,
                build_platform: PlatformWithVirtualPackages::detect_for_platform(
                    build_platform,
                    &virtual_package_override,
                )
                .map_err(|e| {
                    RattlerBuildError::Other(format!("Platform detection error: {}", e))
                })?,
                hash: hash_info,
                variant,
                directories: Directories::builder(
                    &build_name,
                    &safe_recipe_path,
                    &output_dir,
                    &timestamp,
                )
                .no_build_id(no_build_id)
                .build()
                .map_err(|e| RattlerBuildError::Other(format!("Directory setup error: {}", e)))?,
                channels: channels_urls,
                channel_priority: tool_config.channel_priority,
                solve_strategy: SolveStrategy::Highest,
                timestamp,
                subpackages,
                packaging_settings: PackagingSettings::from_args(
                    rattler_conda_types::package::CondaArchiveType::Conda,
                    rattler_conda_types::compression_level::CompressionLevel::Default,
                ),
                store_recipe: false,
                force_colors: false,
                sandbox_config: None,
                exclude_newer: None,
            },
            finalized_dependencies: None,
            finalized_sources: None,
            finalized_cache_dependencies: None,
            finalized_cache_sources: None,
            build_summary: Arc::new(Mutex::new(BuildSummary::default())),
            system_tools: SystemTools::default(),
            extra_meta: None,
        };

        // Extract directory paths before moving output into the async block
        let work_dir = output.build_configuration.directories.work_dir.clone();
        let host_prefix = output.build_configuration.directories.host_prefix.clone();
        let build_prefix = output.build_configuration.directories.build_prefix.clone();
        let build_dir = output.build_configuration.directories.build_dir.clone();
        let recipe_path_out = output.build_configuration.directories.recipe_path.clone();
        let output_dir_out = output.build_configuration.directories.output_dir.clone();

        // Run the debug setup flow with log capture
        let (setup_result, log_buffer) =
            tracing_subscriber::with_log_capture(progress_callback, || {
                run_async_task(async {
                    output
                        .build_configuration
                        .directories
                        .recreate_directories()
                        .map_err(|e| miette::miette!("Failed to create directories: {}", e))?;

                    let output = output
                        .fetch_sources(&tool_config, apply_patch_custom)
                        .await
                        .map_err(|e| miette::miette!("Failed to fetch sources: {}", e))?;

                    let output = output
                        .resolve_dependencies(&tool_config, RunExportsDownload::DownloadMissing)
                        .await
                        .map_err(|e| miette::miette!("Failed to resolve dependencies: {}", e))?;

                    output
                        .install_environments(&tool_config)
                        .await
                        .map_err(|e| miette::miette!("Failed to install environments: {}", e))?;

                    output
                        .create_build_script()
                        .await
                        .map_err(|e| miette::miette!("Failed to create build script: {}", e))?;

                    Ok(())
                })
            });

        let captured_logs = log_buffer
            .lock()
            .map(|buffer| buffer.clone())
            .unwrap_or_default();

        // Check if setup succeeded
        if let Err(err) = setup_result {
            let log_text = if captured_logs.is_empty() {
                String::new()
            } else {
                format!("\n\nSetup log:\n{}", captured_logs.join("\n"))
            };
            return Err(RattlerBuildError::Other(format!("{}{}", err, log_text)).into());
        }

        // Determine build script path
        #[cfg(unix)]
        let build_script = work_dir.join("conda_build.sh");
        #[cfg(windows)]
        let build_script = work_dir.join("conda_build.bat");

        Ok(DebugSessionPy {
            work_dir,
            host_prefix,
            build_prefix,
            build_dir,
            build_script,
            recipe_path: recipe_path_out,
            output_dir: output_dir_out,
            log: captured_logs,
        })
    }

    /// Run the build script and capture stdout/stderr.
    ///
    /// Args:
    ///     trace: If true, run with `bash -ex` (trace mode). Otherwise `bash -e`.
    ///
    /// Returns:
    ///     DebugRunResult with exit_code, stdout, and stderr.
    #[pyo3(signature = (trace=false))]
    fn run(&self, trace: bool) -> PyResult<DebugRunResultPy> {
        let build_env = self.work_dir.join("build_env.sh");
        if !build_env.exists() {
            return Err(RattlerBuildError::Other(format!(
                "build_env.sh not found in {}",
                self.work_dir.display()
            ))
            .into());
        }

        if !self.build_script.exists() {
            return Err(RattlerBuildError::Other(format!(
                "Build script not found: {}",
                self.build_script.display()
            ))
            .into());
        }

        let bash_flag = if trace { "-ex" } else { "-e" };

        let script = format!(
            "cd '{}' && source build_env.sh && bash {} conda_build.sh",
            self.work_dir.display(),
            bash_flag,
        );

        let child = Command::new("bash")
            .arg("-c")
            .arg(&script)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                RattlerBuildError::Other(format!("Failed to spawn build script: {}", e))
            })?;

        let output = child.wait_with_output().map_err(|e| {
            RattlerBuildError::Other(format!("Failed to wait for build script: {}", e))
        })?;

        Ok(DebugRunResultPy {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "DebugSession(work_dir={}, build_script={})",
            self.work_dir.display(),
            self.build_script.display()
        )
    }
}

/// Register the debug module with Python
pub fn register_debug_module(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(py, "debug")?;
    m.add_class::<DebugSessionPy>()?;
    m.add_class::<DebugRunResultPy>()?;
    parent.add_submodule(&m)?;
    Ok(())
}
