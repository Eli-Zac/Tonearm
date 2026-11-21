//! Tries the Tonearm account from a terminal, in a profile folder of its own
//! so the real sign in is left alone:
//!
//!   cargo run -p tonearm-core --example account -- <profile folder> pair <CODE>
//!   cargo run -p tonearm-core --example account -- <profile folder> devices
//!   cargo run -p tonearm-core --example account -- <profile folder> start
//!   cargo run -p tonearm-core --example account -- <profile folder> signout
//!   ... set <key> <value> | get <key> | sync   (settings sync)

use tonearm_core::{account, store};

/// A QR code as a 24-bit BMP: black on white, 12 pixels a module, a blank
/// border of four modules.
fn bmp(qr: &account::Qr) -> Vec<u8> {
    const SCALE: usize = 12;
    const QUIET: usize = 4;
    let side = (qr.size + QUIET * 2) * SCALE;
    let row_bytes = (side * 3).div_ceil(4) * 4;
    let mut out = Vec::with_capacity(54 + row_bytes * side);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + row_bytes * side) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(side as i32).to_le_bytes());
    out.extend_from_slice(&(side as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    // Bottom row first.
    for y in (0..side).rev() {
        let module_y = (y / SCALE).wrapping_sub(QUIET);
        let mut row = vec![255u8; row_bytes];
        for x in 0..side {
            let module_x = (x / SCALE).wrapping_sub(QUIET);
            let dark = module_x < qr.size && module_y < qr.size && qr.dark[module_y * qr.size + module_x];
            if dark {
                row[x * 3..x * 3 + 3].fill(0);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

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
            Ok(p) => {
                println!("code {} ({} s)", p.code, p.expires_in);
                // Also as a picture, to scan from the screen: <profile>/pair.bmp.
                if let Some(qr) = account::pair_qr(&p.code) {
                    let path = std::path::Path::new(arg(0)).join("pair.bmp");
                    std::fs::write(&path, bmp(&qr)).expect("write pair.bmp");
                    println!("picture {}", path.display());
                }
            }
            Err(e) => println!("failed: {e}"),
        },
        "set" => {
            // set <key> <value>: writes a synced setting, as the app would.
            let _ = store::save(arg(2), arg(3));
            println!("set {}", arg(2));
        }
        "get" => println!("{} = {:?}", arg(2), store::load(arg(2))),
        "sync" => println!("{:?}", tonearm_core::settings_sync::sync()),
        "signout" => {
            account::sign_out();
            println!("signed out");
        }
        _ => println!("pair <code> | devices | start | set <key> <value> | get <key> | sync | signout"),
    }
}
