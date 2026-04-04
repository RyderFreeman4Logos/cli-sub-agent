# Project Review Checklist

Common pitfalls and patterns to verify during code review:

- [ ] RAII guards call `finalize()` before `process::exit()` (exit skips Drop)
- [ ] bwrap `--bind` source paths verified to exist before sandbox launch
- [ ] Error paths clean up resources (temp files, locks, cgroup scopes)
- [ ] New public APIs have `/// # Errors` documentation
- [ ] Config structs with `serde(default)` implement `is_default()` check
- [ ] Shell wrapper scripts handle missing commands gracefully (command -v check)
