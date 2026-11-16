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
| Getting the audio stream for a track | Written with yt-dlp, needs testing |
| Windows media keys and media overlay | Written, needs testing on Windows |
| Discord Rich Presence | Written, needs testing with Discord running |
| Last.fm scrobbling with Spotify metadata | Written, needs testing with real keys |
| Animated album covers | Frame player works, no catalogue source yet |

### Audio streams

YouTube has no public API that hands out audio streams, and the ways
around that break often and sit outside YouTube's terms. Tonearm hands the
job to [yt-dlp](https://github.com/yt-dlp/yt-dlp), which keeps up with
those changes. On first play it downloads `yt-dlp.exe` from yt-dlp's GitHub
releases into `%LOCALAPPDATA%	onearm`, checks it against GitHub's SHA-256
digest, and lets it update itself once per start. Some YouTube formats need
a JavaScript runtime, so yt-dlp uses Deno, Node or Bun if one is installed.

The resolver lives behind one trait in `src/resolver.rs`. Set
`TONEARM_TEST_STREAM` to any M4A URL to test the audio path without it.

## Updates

Settings has an Updates card. Tonearm also checks GitHub for a newer release
at start up and shows a toast when one is out. Installing downloads the new
`tonearm.exe`, checks its SHA-256, swaps it in and restarts.

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

Open Settings, then Connections. Each service has its own card.

Release builds carry Tonearm's own Discord application and Last.fm keys.
Press Connect and you are done. Discord needs no sign in. Last.fm opens a
page in your browser where you approve Tonearm.

Spotify needs keys that belong to you: a client ID and client secret from
developer.spotify.com.

### Building with the keys

The keys are not in this repository. The build reads them from the
environment, or from a `keys.env` file next to `Cargo.toml` that git ignores:

```
TONEARM_DISCORD_APP=your application ID from discord.com/developers
TONEARM_LASTFM_KEY=your API key from last.fm/api/account/create
TONEARM_LASTFM_SECRET=the shared secret that comes with it
```

The release workflow reads the same three names from the repository secrets.
A build without them still works. Connections then asks for your own
Discord application ID and Last.fm keys.

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

The icons are from Phosphor (phosphoricons.com), under the MIT licence in
`ui/icons-LICENSE.txt`.
