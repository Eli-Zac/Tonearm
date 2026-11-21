//! Tries the Tonearm account from a terminal, in a profile folder of its own
//! so the real sign in is left alone:
//!
//!   cargo run -p tonearm-core --example account -- <profile folder> pair <CODE>
//!   cargo run -p tonearm-core --example account -- <profile folder> devices
//!   cargo run -p tonearm-core --example account -- <profile folder> start
//!   cargo run -p tonearm-core --example account -- <profile folder> signout

use tonearm_core::{account, store};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    store::set_home(std::path::PathBuf::from(arg(0)));
    println!("server: {}", account::server());
    match arg(1) {
        "pair" => match account::pair_claim(arg(2), "Test computer") {
            Ok(a) => println!("joined {}", a.email),
            Err(e) => println!("failed: {e}"),
        },
        "devices" => match account::devices() {
            Ok(list) => {
                for d in list {
                    println!("{}{}  last seen {}", d.name, if d.current { " (this)" } else { "" }, d.last_seen);
                }
            }
            Err(e) => println!("failed: {e}"),
        },
        "start" => match account::pair_start() {
            Ok(p) => println!("code {} ({} s)", p.code, p.expires_in),
            Err(e) => println!("failed: {e}"),
        },
        "signout" => {
            account::sign_out();
            println!("signed out");
        }
        _ => println!("pair <code> | devices | start | signout"),
    }
}
