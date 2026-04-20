use std::collections::HashMap;

use tokio::process::Command;

const FORBIDDEN_ENV_KEYS: &[&str] = &[
    "LEFTHOOK",
    "LEFTHOOK_BIN",
    "LEFTHOOK_VERBOSE",
    "LEFTHOOK_QUIET",
    "LEFTHOOK_PROFILE",
    "LEFTHOOK_SKIP",
    "LEFTHOOK_EXCLUDE",
    "SKIP",
];

pub(crate) fn sanitize_env_for_codex(cmd: &mut Command) {
    for key in FORBIDDEN_ENV_KEYS {
        cmd.env_remove(key);
    }

    let explicit_prefixed_keys = cmd
        .as_std()
        .get_envs()
        .filter_map(|(key, _)| {
            let key = key.to_string_lossy();
            is_forbidden_env_key_prefix(key.as_ref()).then(|| key.into_owned())
        })
        .collect::<Vec<_>>();
    for key in explicit_prefixed_keys {
        cmd.env_remove(key);
    }

    for (key, _) in std::env::vars_os() {
        if is_forbidden_env_key_prefix(key.to_string_lossy().as_ref()) {
            cmd.env_remove(key);
        }
    }
}

pub(crate) fn sanitize_env_map_for_codex(env: &mut HashMap<String, String>) {
    env.retain(|key, _| !is_forbidden_env_key(key));
}

pub(crate) fn sanitize_args_for_codex(args: &mut Vec<String>) {
    args.retain(|arg| !is_forbidden_arg(arg));
}

fn is_forbidden_env_key(key: &str) -> bool {
    FORBIDDEN_ENV_KEYS.contains(&key) || is_forbidden_env_key_prefix(key)
}

fn is_forbidden_env_key_prefix(key: &str) -> bool {
    key.starts_with("LEFTHOOK_SKIP_") || key.starts_with("LEFTHOOK_EXCLUDE_")
}

fn is_forbidden_arg(arg: &str) -> bool {
    arg == "--no-verify" || arg.starts_with("--no-verify=")
}

#[cfg(test)]
mod tests {
    use super::{sanitize_args_for_codex, sanitize_env_for_codex, sanitize_env_map_for_codex};
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use tokio::process::Command;

    #[test]
    fn sanitize_command_removes_lefthook_env_keys() {
        let mut cmd = Command::new("/bin/true");
        cmd.env("LEFTHOOK", "0");
        cmd.env("LEFTHOOK_SKIP", "pre-commit");
        cmd.env("LEFTHOOK_SKIP_PRE_COMMIT", "1");
        cmd.env("SAFE_ENV", "ok");

        sanitize_env_for_codex(&mut cmd);

        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        let env_map: HashMap<&OsStr, Option<&OsStr>> = envs.into_iter().collect();

        assert_eq!(env_map.get(OsStr::new("LEFTHOOK")), Some(&None));
        assert_eq!(env_map.get(OsStr::new("LEFTHOOK_SKIP")), Some(&None));
        assert_eq!(
            env_map.get(OsStr::new("LEFTHOOK_SKIP_PRE_COMMIT")),
            Some(&None)
        );
        assert_eq!(
            env_map.get(OsStr::new("SAFE_ENV")),
            Some(&Some(OsStr::new("ok")))
        );
    }

    #[test]
    fn sanitize_env_map_removes_prefixed_lefthook_keys() {
        let mut env = HashMap::from([
            ("LEFTHOOK".to_string(), "0".to_string()),
            ("LEFTHOOK_EXCLUDE_PRE_PUSH".to_string(), "1".to_string()),
            ("SAFE_ENV".to_string(), "ok".to_string()),
        ]);

        sanitize_env_map_for_codex(&mut env);

        assert!(!env.contains_key("LEFTHOOK"));
        assert!(!env.contains_key("LEFTHOOK_EXCLUDE_PRE_PUSH"));
        assert_eq!(env.get("SAFE_ENV"), Some(&"ok".to_string()));
    }

    #[test]
    fn sanitize_args_removes_no_verify_variants_only() {
        let mut args = vec![
            "--model".to_string(),
            "gpt-5".to_string(),
            "--no-verify".to_string(),
            "--no-verify=pre-commit".to_string(),
            "-n".to_string(),
            "exec".to_string(),
        ];

        sanitize_args_for_codex(&mut args);

        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "gpt-5".to_string(),
                "-n".to_string(),
                "exec".to_string()
            ]
        );
    }
}
