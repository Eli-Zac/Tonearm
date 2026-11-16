fn main() {
    bake_keys();
    set_icon();
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("slint build failed");
}

/// Puts the logo in the exe, so Explorer, the taskbar, the Start menu
/// shortcut and the media flyout all show it. Redraw the file with
/// `python assets/render_logo.py` after changing assets/logo.svg.
fn set_icon() {
    println!("cargo:rerun-if-changed=assets/tonearm.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/tonearm.ico");
        if let Err(e) = res.compile() {
            // A missing resource compiler should not stop the build.
            println!("cargo:warning=No app icon in the exe: {e}");
        }
    }
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
