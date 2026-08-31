fn configure_fake_gemini_cache(
    temp: &tempfile::TempDir,
    env: &mut HashMap<String, String>,
) {
    let home = temp.path().join("home");
    std::fs::create_dir_all(home.join(".gemini")).expect("create fake Gemini home");
    env.insert(
        "HOME".to_string(),
        home.to_string_lossy().into_owned(),
    );
    let cache_home = temp.path().join("xdg-cache");
    std::fs::create_dir(&cache_home).expect("create fake Gemini cache home");
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        cache_home.to_string_lossy().into_owned(),
    );
}
