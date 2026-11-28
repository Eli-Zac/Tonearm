//! Tries Parties from a terminal, each in a profile folder of its own so the
//! real sign in is left alone. A test server: set TONEARM_ACCOUNT_SERVER
//! (for example http://127.0.0.1:8787 for `npx wrangler dev` in server/).
//!
//!   cargo run -p tonearm-core --example party -- <profile> host <name>
//!       starts a Party (the profile must be signed in) and prints what
//!       happens; typing `state <title>`, `others on|off`, `remove <name>`
//!       or `end` sends that.
//!   cargo run -p tonearm-core --example party -- <profile> look <code>
//!   cargo run -p tonearm-core --example party -- <profile> join <code> <name>
//!       joins and prints what happens; typing `add <video id>`, `next`,
//!       `pause` or `leave` sends that.

use serde_json::json;
use std::io::BufRead;
use std::time::Duration;
use tonearm_core::party::{self, Event, Live};
use tonearm_core::store;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    store::set_home(std::path::PathBuf::from(arg(0)));
    let ticket = match arg(1) {
        "host" => party::start(arg(2)),
        "join" => party::join(arg(2), arg(3)),
        "look" => {
            match party::summary(arg(2)) {
                Ok(s) => println!("{s:#?}"),
                Err(e) => println!("error: {e}"),
            }
            return;
        }
        _ => {
            println!("host <name> | look <code> | join <code> <name>");
            return;
        }
    };
    let ticket = match ticket {
        Ok(t) => t,
        Err(e) => {
            println!("error: {e}");
            return;
        }
    };
    println!("in {} ({}), link {}", ticket.name, ticket.code, party::link(&ticket.code));
    let live = Live::connect(&ticket);

    // Typed lines go to the room.
    let (typed, lines) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if typed.send(line).is_err() {
                break;
            }
        }
    });
    let mut members = Vec::new();
    loop {
        if let Some(event) = live.next_event(Duration::from_millis(200)) {
            println!("event: {event:?}");
            match &event {
                Event::Welcome { members: m, .. } | Event::Members(m) => members = m.clone(),
                Event::Command { from, command, .. } if command == "add" && ticket.host => {
                    // As the app will: the song is queued, then counted.
                    live.send(json!({ "type": "added", "member": from }));
                }
                Event::Removed | Event::Ended(_) => {
                    std::thread::sleep(Duration::from_millis(300));
                    return;
                }
                _ => {}
            }
        }
        while let Ok(line) = lines.try_recv() {
            let (word, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
            let message = match word {
                "state" => json!({ "type": "state", "state": { "song": { "title": rest }, "playing": true } }),
                "others" => json!({ "type": "settings", "others_control": rest == "on" }),
                "remove" => match members.iter().find(|m| m.name == rest) {
                    Some(m) => json!({ "type": "remove", "member": m.id }),
                    None => {
                        println!("no member {rest}");
                        continue;
                    }
                },
                "add" => json!({ "type": "add", "item": { "video_id": rest }, "next": false }),
                "end" | "leave" | "next" | "pause" | "play" | "previous" => json!({ "type": word }),
                _ => {
                    println!("?");
                    continue;
                }
            };
            live.send(message);
        }
    }
}
