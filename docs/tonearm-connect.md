# Tonearm Connect

Plan for controlling playback across the devices signed in to one Tonearm
account: see what each device is playing, control it from another, and move
playback from one device to another without losing the queue or the place in
the song.

The feature is always called **Tonearm Connect**, in the apps, the docs, the
code and commit messages.

## What it should do

- **See:** every device on the account that is online, which one is playing,
  and what it is playing.
- **Control:** from any device, play, pause, skip, go back, seek, change
  volume, shuffle and repeat on the device that is playing, and play a song or
  add it to that device's queue.
- **Move:** send playback to another device. The queue, the song, the place in
  it, shuffle and repeat go with it; the old device stops.
- **Pick up:** a device that opens while another plays shows that playback,
  and can take it over with one tap.

One device on an account plays at a time. The others are remotes for it.

## What exists already

- **Account server** (`server/`): a Cloudflare Worker with D1 at
  `api.tonearm.eli-zac.dev`. Email sign-in, pairing by code or QR, a device
  list (`GET /v1/devices`, with name, kind, model and last seen), and settings
  sync storage (`/v1/settings`, not used by the apps yet). Requests and
  answers only: nothing can be pushed to a device.
- **Core account client** (`crates/tonearm-core/src/account.rs`): sign-in,
  pairing, `devices()`, the device's token in the store.
- **`session::Session`** (`crates/tonearm-core/src/session.rs`): the queue
  (trimmed around the playing song), the index, the position, the volume,
  shuffle and repeat, with `serialize`. This is what a move carries.
- **`queue::Queue`** in core, used by Android; the desktop still has its own
  queue rules in `main.rs`.
- **Casting** on both apps: a Devices panel, and a mode where the player
  controls something else (a cast device). Tonearm Connect's remote mode
  follows the same pattern.
- **`listen::Listen`** and the services thread: Last.fm and Discord, per
  device.

## How it works

### The hub

A Durable Object per account, `ConnectHub`, holds the account's live
connections. Each device that is signed in and open keeps a WebSocket to it.

- `GET /v1/connect` upgrades to a WebSocket. The device's token goes in the
  `Authorization` header, checked as every other route checks it, and the
  Worker hands the socket to the account's hub (`idFromName(account_id)`).
- The hub uses the hibernation WebSocket API, so idle connections cost
  nothing while nobody is playing.
- The hub keeps, in memory and in its own storage: which devices are online,
  which one is active, and the active device's last reported state. A device
  that connects gets that at once, so it can show what is playing without
  waiting.
- The hub only passes messages along and keeps the latest state. Playback
  itself happens on the devices.

D1 stays the source of truth for accounts and devices. Removing a device
there also closes its socket in the hub.

### Messages

JSON, one object per WebSocket message, each with a `type`. Device ids are
the account server's.

From a device to the hub:

| type | what it carries |
| --- | --- |
| `hello` | app version, device kind, whether it can play (a device can be a remote only) |
| `state` | the active device's state: song (video id, title, artists, art, length), playing or paused, position and when it was measured, volume, shuffle, repeat, the next few songs |
| `command` | `to` (a device id) and one of: `play`, `pause`, `next`, `previous`, `seek {ms}`, `volume {0..1}`, `shuffle {on}`, `repeat {mode}`, `play_item {item}`, `add {item, next}` |
| `transfer` | `to`: move playback to that device |
| `hand_off` | the old device's answer to a transfer: the `Session`, playing or not, the position and when it was measured |

From the hub to a device:

| type | what it carries |
| --- | --- |
| `devices` | the online devices, and which one is active. Sent on connect and on every change |
| `state` | the active device's latest state, passed on to everyone else |
| `command` | a command for this device to carry out |
| `give_up` | this device is asked to hand off playback (a transfer away from it) |
| `take_over` | this device is to play the `hand_off` that came with it |
| `error` | why something was refused, for example a transfer to a device that went offline |

The active device sends `state` when something changes (song, play or pause,
a seek, volume, shuffle, repeat, the queue), and every 15 seconds while
playing. Remotes draw the progress bar from the position and its timestamp, so
nothing is sent every second. The server's time comes with every message, so
each device can correct for its own clock.

### Moving playback

1. A remote (or the device itself) sends `transfer {to: B}`.
2. The hub sends `give_up` to the active device A.
3. A pauses, and answers `hand_off` with its `Session`, whether it was
   playing, and the position with its timestamp.
4. The hub sends `take_over` to B, and marks B active.
5. B loads the queue, seeks to the position plus the time the move took, and
   plays if A was playing. It then sends `state`.
6. A stops, keeps nothing playing, and becomes a remote showing B.

If B is offline or does not send `state` within about 10 seconds, the hub
tells A to carry on and sends `error` to whoever asked. Nothing is lost
before B confirms.

A device that nobody has made active (the app just opened) becomes active by
pressing play on itself: it sends `state`, and the hub moves "active" to it
and tells the old active device to pause. Two devices never play at once.

### Last.fm, Discord and history

Only the active device scrobbles, sets "now playing", adds to the YouTube
Music history and records listening. A remote does none of these, so nothing
is counted twice.

Discord can only be shown from the desktop app. A desktop app that is a
remote for a phone can show the phone's song as its Discord status, from the
`state` it receives. This would cover the "Discord presence from the phone via
the account" item on the account plan. It is off unless the user turns it on.

## The apps

### Core (`crates/tonearm-core/src/connect.rs`)

Shared by both apps, as `account.rs` is.

- A background thread that keeps the WebSocket open while the device is
  signed in: it connects, sends `hello`, reconnects with backoff (1 s up to
  60 s) when the connection drops, and stops on sign-out.
- An event channel to the app: devices changed, state received, a command to
  carry out, give up, take over, an error.
- `send_state`, `command`, `transfer` and `hand_off` for the app to call.
- Message types with serde, and tests for encoding and decoding them.
- A WebSocket client that works on Windows and Android: `tungstenite` over the
  TLS the core already uses.

### Desktop

- **Devices panel:** a Tonearm Connect section above Cast. It lists the
  account's online devices with their phone or computer icon, marks the one
  playing, and moves playback to a device when it is clicked. Signed out, it
  says what Tonearm Connect is and links to the Tonearm account page.
- **Remote mode:** while another device plays, the player bar shows that
  device's song, cover, progress and buttons, with "Playing on <device>" under
  it, as casting does. Every button sends a `command`. The menu's Play next and
  Add to queue send `add`. Playing a song or a list from a page asks whether
  to play it here or on that device; the answer can be remembered.
- **Active mode:** the app sends `state` from the places that already update
  the player bar, the taskbar and Discord, and carries out commands through
  the same functions its own buttons call.
- **Cast:** casting is something the active device does. A desktop that is
  casting is still the active device, and its `state` says it is casting to
  which speaker.

### Android

- **Devices panel:** the same Tonearm Connect section as the desktop.
- **Remote mode:** the mini player and Now playing show the remote device and
  send commands. The media notification shows the remote song too, so the
  lock screen and Bluetooth buttons control it.
- **Active mode:** as on the desktop, from the player's existing hooks.
- **Staying reachable:** Android closes the connections of apps in the
  background. While the phone is playing, the playback service is in the
  foreground, so the socket stays open. While the phone is idle in the
  background it is offline to Tonearm Connect, and the list shows it as such.
  So the phone can always control and take over, and can be sent playback
  while the app is open or playing. Waking an idle phone to play needs push
  messages (Firebase Cloud Messaging), which is a later phase, if wanted.

## Security and limits

- Every socket belongs to one device token, checked on connect. A hub only
  ever holds one account's devices, so messages cannot cross accounts.
- Signing out or removing a device closes its socket.
- Messages over 64 KB are refused, and each device is limited in how many
  messages it can send per second. A `hand_off` carries the trimmed session
  (at most 251 songs), which fits.
- The server keeps only the latest state. No history of what was played is
  stored.
- Cloudflare's free plan includes Durable Objects with SQLite storage. Check
  its request and duration limits before launch. With hibernation and state
  sent only on changes, one account's use should be small.

## Phases

Each phase can be used on its own and is tested before the next starts.

1. **Hub and core client.** `ConnectHub`, `/v1/connect`, the messages, and
   `connect.rs` with tests. Tried with two desktop copies under their own
   profiles (`TONEARM_PROFILE`) and a small command-line client in the core's
   examples: devices appear and disappear, state passes along, a transfer
   completes and times out correctly.
2. **Desktop.** The Devices panel section, active mode (state and commands),
   remote mode, and moving playback. Tried between two desktop copies, then
   a real PC and a laptop.
3. **Android.** The same on the phone, with the foreground service rules.
   Tried phone to PC, PC to phone (app open, then app playing in the
   background), and on the emulator.
4. **The rest.** Only the active device scrobbles; Discord from a remote
   device; the "play here or there" question; error and offline messages;
   reconnecting after sleep and network changes.
5. **Later, if wanted.** Push messages to wake an idle phone; volume from the
   hardware buttons while a remote; a web remote.

Rough size: phase 1 about one session, phases 2 and 3 about one each,
phase 4 about one, plus testing on real devices throughout.

## Open questions

- Should a device that opens while another plays offer to take over (a bar at
  the bottom: "Playing on Pixel. Play here?"), or only show it?
- When a song is picked on a remote, play it on the active device by default,
  or ask the first time?
- Should Tonearm Connect be on for every signed-in device, or have a switch in
  Settings?
- Does the old device keep the queue (paused) after a move, or clear it?
