fn main() {
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
