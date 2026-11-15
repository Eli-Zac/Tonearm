# Tonearm

A native Windows client for YouTube Music, written in Rust.

Tonearm has no web view and no browser engine. It draws its own UI with a
software renderer and talks to YouTube Music over plain HTTPS, so it starts
fast and stays small in memory.

Tonearm is unofficial. Google does not endorse it.

## Status

This is an early build. Here is what works and what does not.

| Area | State |
| --- | --- |
| Home, Explore, search, artist, album and playlist pages | Works |
| Sign in with a browser cookie, library, likes | Written, needs testing with a real account |
| Queue, radio, autoplay when the queue ends | Works |
| Syllable synced lyrics with Apple Music style animation | Works |
| Audio engine, streaming AAC decode, seek, volume | Written, needs testing on Windows |
| Getting the audio stream for a track | Missing, see below |
| Windows media keys and media overlay | Written, needs testing on Windows |
| Discord Rich Presence | Written, needs testing with Discord running |
| Last.fm scrobbling with Spotify metadata | Written, needs testing with real keys |
| Animated album covers | Frame player works, no catalogue source yet |

### The missing piece: audio streams

YouTube has no public API that hands out audio streams. Every client that
plays music without a web view gets them one of two ways. It imitates
another YouTube client, or it runs YouTube's own player script to unlock the
stream links. Both break often and both sit outside YouTube's terms.

That choice is yours to make, so Tonearm keeps it behind one trait in
`src/resolver.rs`. Implement `Resolver::resolve` and playback works. Until
then you can set `TONEARM_TEST_STREAM` to any M4A URL to test the audio path.

## Download

Grab `tonearm.exe` from the Releases page on GitHub. The file is unsigned,
so Windows SmartScreen warns on first run. Pick "More info", then "Run
anyway".

To cut a release, open the Actions tab, pick Release, press "Run workflow"
and enter a version such as `v0.1.1`. GitHub builds the exe on Windows and
attaches it. Pushing a version tag does the same.

## Build

You need a recent stable Rust.

```bash
cargo build --release
```

The binary lands in `target/release/tonearm.exe`.

To cross compile from Linux, install `mingw-w64` and run this.

```bash
cargo build --release --target x86_64-pc-windows-gnu
```

## Sign in

Tonearm cannot show the Google sign in page, because that needs a browser.
Use your browser's session instead.

1. Sign in to music.youtube.com in your browser.
2. Open the developer tools and pick the Network tab.
3. Click any request to music.youtube.com.
4. Copy the value of the `Cookie` request header.
5. Paste it into Settings in Tonearm.

Tonearm encrypts the cookie with Windows DPAPI and sends it only to
music.youtube.com.

## Lyrics

Tonearm asks the same public lyric services the BitChord client asks, in the
same order, and stops at the first one that has the track.

1. lrc.red, syllable timed
2. BetterLyrics, syllable timed
3. LRCLIB, line timed
4. YouTube Music's own lyrics, plain text

BitChord also asks LyricsPlus. Its community mirrors were down when we built
this, so Tonearm does not ask it yet.

## Discord, Last.fm and Spotify

Each service needs keys that belong to you. Enter them in Settings.

| Service | What you need | Where to get it |
| --- | --- | --- |
| Discord | Application ID | discord.com/developers |
| Last.fm | API key and shared secret | last.fm/api/account/create |
| Spotify | Client ID and client secret | developer.spotify.com |

With Spotify keys set, Tonearm looks each track up on Spotify and reports
Spotify's title, artist and album to Last.fm and Discord. Without a sure
match it reports the YouTube Music names.

A track scrobbles after it has played for half its length or four minutes.

## How it stays small

* No web view, no JavaScript engine, no GPU context
* Slint's software renderer, which repaints only what changed
* Blocking HTTP on a few worker threads, no async runtime
* Windows' own TLS stack instead of a bundled one
* Thumbnails fetched at the size we draw, in a 12 MB cache
* One track buffered in memory at a time
* The audio device opens on first play
* The UI ticks four times a second, and faster only while lyrics move

## Layout

| Path | What it holds |
| --- | --- |
| `ui/app.slint` | The whole interface |
| `src/main.rs` | App state and wiring |
| `src/innertube/` | YouTube Music API client and parsers |
| `src/audio.rs` | Streaming download, decode and output |
| `src/resolver.rs` | The stream resolver trait |
| `src/lyrics.rs` | Lyric services and TTML parsing |
| `src/services.rs` | Discord, Last.fm and Spotify |
| `src/motion.rs` | Animated cover frames |
| `src/media_keys.rs` | Windows media keys |
| `src/store.rs` | Encrypted storage for secrets |

## Licences

Slint is free for desktop apps under its royalty free licence, which asks
for attribution, or under GPLv3. Pick the one that fits before you ship.

The icons are from Lucide, under the ISC licence in `ui/icons-LICENSE.txt`.
