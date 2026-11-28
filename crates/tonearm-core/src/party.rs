//! Parties: a shared queue that a host's device plays and others add to.
//! The plan is `docs/party.md`; the server's side is `server/src/party.ts`.
//!
//! Starting a Party needs the Tonearm account; looking one up and joining
//! don't. Each member gets a token for that one Party, and keeps a live
//! connection ([`Live`]) to the Party's room on the server, which passes the
//! host's state to the guests and the guests' requests to the host.

use crate::account::{self, Qr};
use crate::innertube::agent;
use serde_json::{json, Value};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Where Party links point. The site opens the app (an Android App Link)
/// or, without the app, shows a page with the Party and a download link.
pub const LINK_BASE: &str = "https://tonearm.eli-zac.dev/party/";

/// The link for a Party's code, `K7Q-M4P`.
pub fn link(code: &str) -> String {
    format!("{LINK_BASE}{}", code.trim())
}

/// The QR code for a Party: its link, so a phone's own camera app can open it.
pub fn qr(code: &str) -> Option<Qr> {
    let made = qrcode::QrCode::new(link(code).as_bytes()).ok()?;
    Some(Qr {
        size: made.width(),
        dark: made.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect(),
    })
}

/// The Party code in a scan, a pasted link or a typed code, as `K7Q-M4P`.
/// None for anything else.
pub fn code_from(text: &str) -> Option<String> {
    let text = text.trim();
    let code = if let Some(rest) = text.strip_prefix(LINK_BASE).or_else(|| text.strip_prefix("tonearm://party/")) {
        rest.split(['?', '#', '/']).next()?
    } else if text.contains("://") {
        return None;
    } else {
        text
    };
    let clean: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    (clean.len() == 6).then(|| format!("{}-{}", &clean[..3], &clean[3..]))
}

/// A Party this device is in: which one, and its token there.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticket {
    pub code: String,
    pub name: String,
    /// This device's member id in the Party.
    pub member_id: String,
    /// Good for this Party only.
    pub token: String,
    pub host: bool,
}

/// What anyone can see of a Party before joining.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub code: String,
    pub name: String,
    pub host: String,
    pub members: u32,
    pub others_control: bool,
    /// The host's last state, as the host sent it.
    pub state: Value,
}

/// Someone in a Party.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub host: bool,
    /// Songs they have added.
    pub added: u32,
    pub online: bool,
}

/// A song as it travels in a Party: a guest's add, the host's queue.
pub fn song_json(item: &crate::innertube::model::Item) -> Value {
    json!({
        "video_id": item.video_id,
        "title": item.title,
        "subtitle": item.subtitle,
        "artists": item.artists.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "art": item.thumb,
        "duration": item.duration,
        "explicit": item.explicit,
    })
}

/// A song from a Party message. None without a video id.
pub fn song_from(v: &Value) -> Option<crate::innertube::model::Item> {
    let video_id = v["video_id"].as_str().filter(|id| !id.is_empty())?.to_string();
    let artists: Vec<(String, Option<String>)> = v["artists"]
        .as_array()
        .map(|a| a.iter().filter_map(|n| n.as_str()).map(|n| (n.to_string(), None)).collect())
        .unwrap_or_default();
    let subtitle = match text(v, "subtitle") {
        s if s.is_empty() => artists.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "),
        s => s,
    };
    Some(crate::innertube::model::Item {
        kind: crate::innertube::model::Kind::Song,
        title: text(v, "title"),
        subtitle,
        thumb: text(v, "art"),
        duration: text(v, "duration"),
        video_id: Some(video_id),
        artists,
        explicit: v["explicit"].as_bool().unwrap_or(false),
        ..Default::default()
    })
}

/// A request to the server with no account sign-in: a Party's own token, or none.
fn call(method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> Result<Value, String> {
    let url = format!("{}{path}", account::server());
    let mut request = agent(Duration::from_secs(20)).request(method, &url);
    if let Some(token) = token {
        request = request.set("authorization", &format!("Bearer {token}"));
    }
    let result = match body {
        Some(body) => request.send_json(body),
        None => request.call(),
    };
    match result {
        Ok(response) => response.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, response)) => {
            let message = response.into_json::<Value>().ok().and_then(|v| v["error"].as_str().map(str::to_string));
            Err(message.unwrap_or_else(|| format!("The server answered {code}")))
        }
        Err(e) => Err(format!("Couldn't reach the Tonearm server: {e}")),
    }
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

fn ticket(v: &Value, host: bool) -> Result<Ticket, String> {
    let ticket = Ticket {
        code: text(v, "code"),
        name: text(v, "name"),
        member_id: text(v, "member_id"),
        token: text(v, "token"),
        host,
    };
    if ticket.code.is_empty() || ticket.token.is_empty() {
        return Err("The server's answer had no Party in it".into());
    }
    Ok(ticket)
}

/// Starts a Party with this device as its host. `host_name` is how the host
/// shows to guests; the Party is named after it ("Eli's Party").
pub fn start(host_name: &str) -> Result<Ticket, String> {
    let token = account::token().ok_or("Sign in to your Tonearm account to start a Party")?;
    let answer = account::call("POST", "/v1/party", Some(&token), Some(json!({ "host_name": host_name })))?;
    ticket(&answer, true)
}

/// What a Party is, from its code, without joining.
pub fn summary(code: &str) -> Result<Summary, String> {
    let code = code_from(code).ok_or("That isn't a Party code")?;
    let v = call("GET", &format!("/v1/party/{code}"), None, None)?;
    Ok(Summary {
        code: text(&v, "code"),
        name: text(&v, "name"),
        host: text(&v, "host"),
        members: v["members"].as_u64().unwrap_or(0) as u32,
        others_control: v["others_control"].as_bool().unwrap_or(false),
        state: v["state"].clone(),
    })
}

/// Joins a Party as a guest, shown to the others as `name`.
pub fn join(code: &str, name: &str) -> Result<Ticket, String> {
    let code = code_from(code).ok_or("That isn't a Party code")?;
    let answer = call("POST", &format!("/v1/party/{code}/join"), None, Some(json!({ "name": name })))?;
    ticket(&answer, false)
}

/// Ends a Party for everyone. Host only.
pub fn end(ticket: &Ticket) -> Result<(), String> {
    call("DELETE", &format!("/v1/party/{}", ticket.code), Some(&ticket.token), None).map(|_| ())
}

/// Something that happened in the Party, from the live connection.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Connected (again): everything as it stands now.
    Welcome { name: String, others_control: bool, members: Vec<Member>, state: Value },
    /// Someone joined, left, came online or went away, or added a song.
    Members(Vec<Member>),
    /// The host's new state. Guests only.
    State(Value),
    /// The host changed whether guests may control playback.
    Settings { others_control: bool },
    /// A guest's request, for the host to carry out: "add" (with `item` and
    /// `next` in `args`), "play", "pause", "next", "previous", "seek"
    /// (`ms`), "move" (`from`, `to`) or "play_item" (`item`). Host only.
    Command { from: String, from_name: String, command: String, args: Value },
    /// The server refused something this device sent.
    Refused(String),
    /// The connection dropped; it is being made again.
    Reconnecting,
    /// The host removed this device from the Party. The connection stops.
    Removed,
    /// The Party is over (`reason`), or no longer exists. The connection stops.
    Ended(String),
}

fn members(v: &Value) -> Vec<Member> {
    v.as_array()
        .map(|list| {
            list.iter()
                .map(|m| Member {
                    id: text(m, "id"),
                    name: text(m, "name"),
                    host: m["host"].as_bool().unwrap_or(false),
                    added: m["added"].as_u64().unwrap_or(0) as u32,
                    online: m["online"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A message from the room as an event. None for anything unknown.
fn event(v: &Value) -> Option<Event> {
    Some(match v["type"].as_str()? {
        "welcome" => Event::Welcome {
            name: text(v, "name"),
            others_control: v["others_control"].as_bool().unwrap_or(false),
            members: members(&v["members"]),
            state: v["state"].clone(),
        },
        "members" => Event::Members(members(&v["members"])),
        "state" => Event::State(v["state"].clone()),
        "settings" => Event::Settings { others_control: v["others_control"].as_bool().unwrap_or(false) },
        "command" => Event::Command {
            from: text(v, "from"),
            from_name: text(v, "from_name"),
            command: text(v, "command"),
            args: v["args"].clone(),
        },
        "error" => Event::Refused(text(v, "error")),
        "removed" => Event::Removed,
        "ended" => Event::Ended(text(v, "reason")),
        _ => return None,
    })
}

/// The live connection to a Party's room. It reconnects by itself (1 s,
/// doubling up to 60 s) until the Party ends, this device is removed, or it
/// is dropped. Messages sent while it is reconnecting are dropped, except the
/// host's latest `state`, which is sent again on reconnecting.
pub struct Live {
    out: Sender<Value>,
    events: std::sync::Mutex<Receiver<Event>>,
    stop: Arc<AtomicBool>,
}

impl Live {
    pub fn connect(ticket: &Ticket) -> Live {
        let (out, outgoing) = mpsc::channel();
        let (incoming, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let ticket = ticket.clone();
        let quit = stop.clone();
        thread::Builder::new()
            .name("party".into())
            .spawn(move || run(ticket, outgoing, incoming, quit))
            .ok();
        Live { out, events: std::sync::Mutex::new(events), stop }
    }

    /// Sends one message to the room: `{"type": "add", ...}` and so on.
    pub fn send(&self, message: Value) {
        let _ = self.out.send(message);
    }

    /// The host's state, for the guests.
    pub fn send_state(&self, state: Value) {
        self.send(json!({ "type": "state", "state": state }));
    }

    /// Events since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().map(|e| e.try_iter().collect()).unwrap_or_default()
    }

    /// Waits up to `wait` for the next event.
    pub fn next_event(&self, wait: Duration) -> Option<Event> {
        self.events.lock().ok()?.recv_timeout(wait).ok()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A connection to the server, with or without TLS.
enum Stream {
    Plain(TcpStream),
    #[cfg(not(target_os = "android"))]
    Tls(native_tls::TlsStream<TcpStream>),
    #[cfg(target_os = "android")]
    Tls(rustls::StreamOwned<rustls::ClientConnection, TcpStream>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            #[cfg(not(target_os = "android"))]
            Stream::Tls(s) => s.get_ref(),
            #[cfg(target_os = "android")]
            Stream::Tls(s) => &s.sock,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

#[cfg(not(target_os = "android"))]
fn secure(host: &str, tcp: TcpStream) -> Result<Stream, String> {
    let connector = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    connector.connect(host, tcp).map(Stream::Tls).map_err(|e| format!("secure connection failed: {e}"))
}

#[cfg(target_os = "android")]
fn secure(host: &str, mut tcp: TcpStream) -> Result<Stream, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let mut connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    while connection.is_handshaking() {
        connection.complete_io(&mut tcp).map_err(|e| format!("secure connection failed: {e}"))?;
    }
    Ok(Stream::Tls(rustls::StreamOwned::new(connection, tcp)))
}

/// Why a connection attempt ended.
enum Stop {
    /// Try again after a wait.
    Retry(String),
    /// Don't: the Party is gone, or this device was removed.
    Done,
}

fn run(ticket: Ticket, outgoing: Receiver<Value>, events: Sender<Event>, stop: Arc<AtomicBool>) {
    let mut wait = Duration::from_secs(1);
    let mut last_state: Option<Value> = None;
    let mut first = true;
    while !stop.load(Ordering::Relaxed) {
        if !first {
            let _ = events.send(Event::Reconnecting);
        }
        first = false;
        let started = Instant::now();
        match session(&ticket, &outgoing, &events, &stop, &mut last_state) {
            Stop::Done => return,
            Stop::Retry(why) => eprintln!("party: connection dropped ({why})"),
        }
        // A connection that lasted a while starts the waits over.
        if started.elapsed() > Duration::from_secs(60) {
            wait = Duration::from_secs(1);
        }
        let until = Instant::now() + wait;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
        wait = (wait * 2).min(Duration::from_secs(60));
    }
}

/// One connection, until it drops or the Party ends.
fn session(
    ticket: &Ticket,
    outgoing: &Receiver<Value>,
    events: &Sender<Event>,
    stop: &AtomicBool,
    last_state: &mut Option<Value>,
) -> Stop {
    let server = account::server();
    let (tls, rest) = match server.split_once("://") {
        Some(("https", rest)) => (true, rest),
        Some(("http", rest)) => (false, rest),
        _ => return Stop::Retry(format!("bad server address {server}")),
    };
    let host_port = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(if tls { 443 } else { 80 })),
        None => (host_port.to_string(), if tls { 443 } else { 80 }),
    };
    let Some(addr) = (host.as_str(), port).to_socket_addrs().ok().and_then(|mut a| a.next()) else {
        return Stop::Retry(format!("couldn't find {host}"));
    };
    let tcp = match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
        Ok(tcp) => tcp,
        Err(e) => return Stop::Retry(e.to_string()),
    };
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = tcp.set_nodelay(true);
    let stream = if tls {
        match secure(&host, tcp) {
            Ok(s) => s,
            Err(e) => return Stop::Retry(e),
        }
    } else {
        Stream::Plain(tcp)
    };

    let scheme = if tls { "wss" } else { "ws" };
    let url = format!("{scheme}://{host_port}/v1/party/{}/live", ticket.code);
    let request = match tungstenite::http::Request::builder()
        .uri(&url)
        .header("host", host_port)
        .header("authorization", format!("Bearer {}", ticket.token))
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", tungstenite::handshake::client::generate_key())
        .body(())
    {
        Ok(r) => r,
        Err(e) => return Stop::Retry(e.to_string()),
    };
    let mut socket = match tungstenite::client(request, stream) {
        Ok((socket, _)) => socket,
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            // Gone or not ours any more: no point trying again.
            if matches!(response.status().as_u16(), 401 | 404) {
                let _ = events.send(Event::Ended("The Party has ended".into()));
                return Stop::Done;
            }
            return Stop::Retry(format!("the server answered {}", response.status()));
        }
        Err(e) => return Stop::Retry(e.to_string()),
    };
    // Short reads from here, so sending isn't held up waiting for messages.
    let _ = socket.get_ref().tcp().set_read_timeout(Some(Duration::from_millis(100)));

    // The host's state again after a reconnect, so guests don't miss a change.
    if let Some(state) = last_state.clone() {
        let _ = socket.send(tungstenite::Message::text(json!({ "type": "state", "state": state }).to_string()));
    }
    let mut pinged = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            let _ = socket.flush();
            return Stop::Done;
        }
        loop {
            match outgoing.try_recv() {
                Ok(message) => {
                    if message["type"] == "state" {
                        *last_state = Some(message["state"].clone());
                    }
                    if let Err(e) = socket.send(tungstenite::Message::text(message.to_string())) {
                        return Stop::Retry(e.to_string());
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    return Stop::Done;
                }
            }
        }
        // The room answers "ping" with "pong" without waking up; it keeps
        // the connection open through routers that drop quiet ones.
        if pinged.elapsed() > Duration::from_secs(25) {
            pinged = Instant::now();
            if let Err(e) = socket.send(tungstenite::Message::text("ping")) {
                return Stop::Retry(e.to_string());
            }
        }
        match socket.read() {
            Ok(tungstenite::Message::Text(raw)) => {
                if raw.as_str() == "pong" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(raw.as_str()) else { continue };
                if let Some(e) = event(&v) {
                    let done = matches!(e, Event::Removed | Event::Ended(_));
                    let _ = events.send(e);
                    if done {
                        return Stop::Done;
                    }
                }
            }
            Ok(tungstenite::Message::Close(frame)) => {
                if frame.as_ref().is_some_and(|f| u16::from(f.code) == 4001) {
                    let _ = events.send(Event::Removed);
                    return Stop::Done;
                }
                return Stop::Retry("closed".into());
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Stop::Retry(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_and_its_code_go_both_ways() {
        assert_eq!(link("K7Q-M4P"), "https://tonearm.eli-zac.dev/party/K7Q-M4P");
        assert_eq!(code_from(&link("K7Q-M4P")).as_deref(), Some("K7Q-M4P"));
        assert_eq!(code_from("tonearm://party/k7qm4p").as_deref(), Some("K7Q-M4P"));
        assert_eq!(code_from(" k7q m4p ").as_deref(), Some("K7Q-M4P"));
    }

    #[test]
    fn other_links_and_short_codes_are_not_parties() {
        assert_eq!(code_from("https://example.com/party/K7Q-M4P"), None);
        assert_eq!(code_from("tonearm://pair?code=TG2G-P6SE"), None);
        assert_eq!(code_from("K7Q"), None);
    }

    #[test]
    fn a_party_qr_is_square() {
        let qr = qr("K7Q-M4P").unwrap();
        assert_eq!(qr.dark.len(), qr.size * qr.size);
    }

    #[test]
    fn the_rooms_messages_become_events() {
        let welcome = json!({"type": "welcome", "name": "Eli's Party", "others_control": true,
            "members": [{"id": "a", "name": "Eli", "host": true, "added": 0, "online": true}], "state": null});
        match event(&welcome) {
            Some(Event::Welcome { name, others_control, members, .. }) => {
                assert_eq!(name, "Eli's Party");
                assert!(others_control);
                assert_eq!(members[0].name, "Eli");
                assert!(members[0].host && members[0].online);
            }
            other => panic!("{other:?}"),
        }
        let command = json!({"type": "command", "from": "b", "from_name": "Sam", "command": "add",
            "args": {"item": {"video_id": "abc"}, "next": true}});
        assert_eq!(
            event(&command),
            Some(Event::Command {
                from: "b".into(),
                from_name: "Sam".into(),
                command: "add".into(),
                args: json!({"item": {"video_id": "abc"}, "next": true}),
            })
        );
        assert_eq!(event(&json!({"type": "ended", "reason": "bye"})), Some(Event::Ended("bye".into())));
        assert_eq!(event(&json!({"type": "removed"})), Some(Event::Removed));
        assert_eq!(event(&json!({"type": "nonsense"})), None);
    }
}
