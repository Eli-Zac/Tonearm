/// Passes Tonearm's own service keys to the compiler, for `services.rs`.
/// They come from the environment, or from the `keys.env` file of
/// NAME=value lines at the top of the repo, which stays out of git. A build
/// without them still works. It asks the user for keys.
fn main() {
    const NAMES: [&str; 3] = [
        "TONEARM_DISCORD_APP",
        "TONEARM_LASTFM_KEY",
        "TONEARM_LASTFM_SECRET",
    ];
    const FILE: &str = "../../keys.env";
    println!("cargo:rerun-if-changed={FILE}");
    let file = std::fs::read_to_string(FILE).unwrap_or_default();
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
