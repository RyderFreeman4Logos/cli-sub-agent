#[test]
fn test_build_memory_section_from_mempal_binary_wraps_context() {
    let temp = tempdir().expect("create tempdir");
    let script_path = temp.path().join("mempal-fake.sh");
    let mut script = fs::File::create(&script_path).expect("create fake mempal");
    writeln!(
        script,
        "#!/bin/sh\nprintf 'mempal remembered %s in %s\\n' \"$6\" \"$3\"\n"
    )
    .expect("write fake mempal");
    drop(script);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("chmod");
    }

    let section = build_memory_section_from_mempal_binary(
        &script_path,
        "review routing",
        temp.path(),
        200,
        Duration::from_secs(5),
    )
    .expect("mempal section");

    assert!(section.contains("<!-- CSA:MEMORY -->"));
    assert!(section.contains("mempal remembered review routing"));
    assert!(section.contains(temp.path().to_string_lossy().as_ref()));
    assert!(section.contains("<!-- CSA:MEMORY:END -->"));
}

#[test]
fn test_build_memory_section_from_mempal_uses_detected_binary() {
    let temp = tempdir().expect("create tempdir");
    let script_path = temp.path().join("mempal-fake.sh");
    let mut script = fs::File::create(&script_path).expect("create fake mempal");
    writeln!(
        script,
        "#!/bin/sh\nif [ \"$1\" = \"context\" ]; then\n  printf 'detected mempal context for %s\\n' \"$6\"\n  exit 0\nfi\nexit 64\n"
    )
    .expect("write fake mempal");
    drop(script);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("chmod");
    }

    let section = build_memory_section_from_mempal_with_detector(
        "prompt routing",
        temp.path(),
        200,
        || Some(script_path),
    )
    .expect("mempal section");

    assert!(section.contains("detected mempal context for prompt routing"));
    assert!(section.contains("<!-- CSA:MEMORY -->"));
    assert!(section.contains("<!-- CSA:MEMORY:END -->"));
}

#[test]
fn test_build_memory_section_from_mempal_binary_returns_none_on_timeout() {
    let temp = tempdir().expect("create tempdir");
    let script_path = temp.path().join("mempal-sleep.sh");
    let mut script = fs::File::create(&script_path).expect("create fake mempal");
    writeln!(script, "#!/bin/sh\nsleep 2\n").expect("write fake mempal");
    drop(script);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("chmod");
    }

    let section = build_memory_section_from_mempal_binary(
        &script_path,
        "review routing",
        temp.path(),
        200,
        Duration::from_millis(100),
    );

    assert!(section.is_none());
}
