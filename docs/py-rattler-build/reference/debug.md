# Debug Session

Interactive debugging for recipe builds.

You can import `DebugSession` and `DebugRunResult` from `rattler_build`:

```python
from rattler_build import DebugSession, DebugRunResult
```

## `DebugSession`

::: rattler_build.DebugSession
    options:
        members:
            - setup
            - run
            - work_dir
            - host_prefix
            - build_prefix
            - build_dir
            - build_script
            - recipe_path
            - output_dir
            - log

## `DebugRunResult`

::: rattler_build.DebugRunResult
    options:
        members:
            - exit_code
            - stdout
            - stderr
