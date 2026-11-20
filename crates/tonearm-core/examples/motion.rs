//! Looks an album's moving cover up on Apple Music and reports each step,
//! without the cache: `cargo run -p tonearm-core --example motion -- <album> <artist>`.
//! The same as the desktop's `--probe motion`, without building the app.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    tonearm_core::animated_art::probe(arg(0), arg(1));
}
