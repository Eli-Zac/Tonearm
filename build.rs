fn main() {
    bake_keys();
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("slint build failed");
}

/// Passes Tonearm's own service keys to the compiler. They come from the
/// environment, or from a `keys.env` file of NAME=value lines that stays out
/// of git. A build without them still works. It asks the user for keys.
fn bake_keys() {
    const NAMES: [&str; 3] = [
        "TONEARM_DISCORD_APP",
        "TONEARM_LASTFM_KEY",
        "TONEARM_LASTFM_SECRET",
    ];
    println!("cargo:rerun-if-changed=keys.env");
    let file = std::fs::read_to_string("keys.env").unwrap_or_default();
    for name in NAMES {
        println!("cargo:rerun-if-env-changed={name}");
        let from_file = file.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.trim().to_string())
        });
        let value = std::env::var(name).ok().or(from_file).unwrap_or_default();
        if !value.is_empty() {
            println!("cargo:rustc-env={name}={value}");
        }
    }
}
