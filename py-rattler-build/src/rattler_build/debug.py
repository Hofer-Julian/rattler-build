"""
Interactive debug session for iterating on recipe builds.

This module provides the ability to set up a debug environment from a rendered
variant — resolving dependencies, fetching sources, installing environments,
and creating the build script — without running the build. You can then
repeatedly run (and re-run) the build script, inspecting stdout/stderr after
each iteration.
"""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from rattler_build._rattler_build import debug as _debug
from rattler_build.tool_config import ToolConfiguration

if TYPE_CHECKING:
    from rattler_build.progress import ProgressCallback
    from rattler_build.render import RenderedVariant


class DebugRunResult:
    """Result of running a build script in a debug session.

    Attributes:
        exit_code: Exit code of the build script process.
        stdout: Captured standard output.
        stderr: Captured standard error.
    """

    def __init__(self, inner: _debug.DebugRunResult) -> None:
        self._inner = inner

    @property
    def exit_code(self) -> int:
        """Exit code of the build script."""
        return self._inner.exit_code

    @property
    def stdout(self) -> str:
        """Captured stdout from the build script."""
        return self._inner.stdout

    @property
    def stderr(self) -> str:
        """Captured stderr from the build script."""
        return self._inner.stderr

    def __repr__(self) -> str:
        return repr(self._inner)


class DebugSession:
    """Interactive debug session for iterating on a recipe build.

    A debug session sets up the full build environment (resolves dependencies,
    fetches sources, installs environments, creates build script) without
    actually running the build. This allows you to inspect the environment
    and iteratively run the build script.

    Example:
        ```python
        from rattler_build import DebugSession

        # After rendering a recipe
        session = DebugSession.setup(rendered_variant=variants[0])

        # Inspect paths
        print(session.work_dir)
        print(session.build_script)

        # Run the build script
        result = session.run(trace=True)
        print(result.exit_code)
        print(result.stderr)
        ```
    """

    def __init__(self, inner: _debug.DebugSession) -> None:
        self._inner = inner

    @classmethod
    def setup(
        cls,
        rendered_variant: RenderedVariant,
        channels: list[str] | None = None,
        output_dir: str | Path | None = None,
        tool_config: ToolConfiguration | None = None,
        no_build_id: bool = True,
        progress_callback: ProgressCallback | None = None,
    ) -> DebugSession:
        """Set up a debug session from a rendered variant.

        This resolves dependencies, fetches sources, installs environments,
        and creates the build script — but does NOT run the build.

        Args:
            rendered_variant: A rendered variant from ``recipe.render()``.
            channels: Channels to use for dependency resolution.
                Defaults to ``["conda-forge"]``.
            output_dir: Directory for build output.
                Defaults to ``<recipe_dir>/output``.
            tool_config: Tool configuration. Defaults to ``ToolConfiguration()``.
            no_build_id: Don't include build ID in directory names.
            progress_callback: Optional progress callback for setup events.

        Returns:
            A configured DebugSession ready for ``run()``.
        """
        if channels is None:
            channels = ["conda-forge"]

        if output_dir is not None:
            output_dir = Path(output_dir)
            output_dir.mkdir(parents=True, exist_ok=True)

        if tool_config is None:
            tool_config = ToolConfiguration()

        inner = _debug.DebugSession.setup(
            rendered_variant=rendered_variant._inner,
            channels=channels,
            output_dir=output_dir,
            tool_config=tool_config._inner,
            recipe_path=rendered_variant.recipe_path,
            no_build_id=no_build_id,
            progress_callback=progress_callback,
        )
        return cls(inner)

    @property
    def work_dir(self) -> Path:
        """Work directory where sources are extracted."""
        return Path(self._inner.work_dir)

    @property
    def host_prefix(self) -> Path:
        """Host prefix directory."""
        return Path(self._inner.host_prefix)

    @property
    def build_prefix(self) -> Path:
        """Build prefix directory."""
        return Path(self._inner.build_prefix)

    @property
    def build_dir(self) -> Path:
        """Build directory (parent of work_dir)."""
        return Path(self._inner.build_dir)

    @property
    def build_script(self) -> Path:
        """Path to the build script (conda_build.sh or conda_build.bat)."""
        return Path(self._inner.build_script)

    @property
    def recipe_path(self) -> Path:
        """Path to the recipe file."""
        return Path(self._inner.recipe_path)

    @property
    def output_dir(self) -> Path:
        """Output directory for built packages."""
        return Path(self._inner.output_dir)

    @property
    def log(self) -> list[str]:
        """Captured log messages from the setup phase."""
        return self._inner.log

    def run(self, trace: bool = False) -> DebugRunResult:
        """Run the build script and capture stdout/stderr.

        This sources ``build_env.sh`` and then runs the build script.
        Can be called multiple times for iterative debugging.

        Args:
            trace: If True, run with ``bash -ex`` (trace mode).
                Otherwise ``bash -e``.

        Returns:
            DebugRunResult with exit_code, stdout, and stderr.
        """
        inner_result = self._inner.run(trace=trace)
        return DebugRunResult(inner_result)

    def __repr__(self) -> str:
        return repr(self._inner)
