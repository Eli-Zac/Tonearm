# Tonearm

A native Windows client for YouTube Music, written in Rust.

Tonearm has no web view and no browser engine. It draws its own UI with
[Slint](https://slint.dev) and Skia, and talks to YouTube Music over plain
HTTPS, so it starts fast and stays small in memory.

> [!IMPORTANT]
> Tonearm is an unofficial, independent project. It is not affiliated with,
> endorsed by or sponsored by Google, YouTube or Spotify. YouTube Music is a
> trademark of Google LLC, and Spotify is a trademark of Spotify AB. Using
> Tonearm may go against YouTube's Terms of Service, and you use it at your
> own risk.

## Features

* Home, Explore, search, and artist, album and playlist pages
* Your library, liked songs and playlists, once you sign in
* A queue you can drag to reorder, radio, smart shuffle, and autoplay when
  the queue runs out
* Crossfade, volume levelling and a sleep timer
* Synced lyrics, timed by the syllable where the source allows, with duet
  sides and out of focus lines
* A Now playing panel with animated covers and Spotify Canvas clips
* Songs kept on your computer to play offline, and a cache so songs you
  play again start at once
* Picks up where you left off: the queue, the song and the position
* Windows media keys, the media flyout, and play controls under the
  taskbar preview
* Closes to the tray and keeps playing, if you want it to
* Discord Rich Presence and Last.fm scrobbling
* In-app updates

Tonearm is young. Expect rough edges, and please report what breaks.

## Install

Download `tonearm-setup.exe` from the
[latest release](../../releases/latest) and run it. It installs just for
you, in `%LOCALAPPDATA%\Programs\Tonearm`, and needs no admin rights. You
can uninstall it from Settings > Apps like any other app.

The installer is not code signed, so Windows SmartScreen may warn the
first time. Pick "More info", then "Run anyway".

### Updates

Tonearm checks for a new release at start up and shows a note when one is
out. Settings has an Updates card too. Installing downloads the new
installer, checks its SHA-256 against the digest GitHub publishes, then
closes Tonearm, installs the update and starts it again.

## Sign in

Tonearm cannot show Google's sign in page, because that needs a browser.
It uses your browser's session instead.

1. Sign in to music.youtube.com in your browser.
2. Open the developer tools and pick the Network tab.
3. Click any request to music.youtube.com.
4. Copy the value of the `Cookie` request header.
5. Paste it into Settings in Tonearm.

> [!WARNING]
> That cookie gives full access to your Google account. Never share it or
> paste it anywhere else.

Tonearm encrypts the cookie with Windows DPAPI, so only your Windows
account can read it, and sends it only to music.youtube.com.

## How playback works

YouTube has no public API that hands out audio streams, and the ways
around that change often. Tonearm hands the job to
[yt-dlp](https://github.com/yt-dlp/yt-dlp), which keeps up with those
changes. On first play it downloads `yt-dlp.exe` from yt-dlp's GitHub
releases into `%LOCALAPPDATA%\tonearm`, checks it against GitHub's SHA-256
digest, and lets it update itself once per start. Some YouTube formats need
a JavaScript runtime, so yt-dlp uses Deno, Node or Bun if one is installed.

The resolver lives behind one trait in `src/resolver.rs`. Set
`TONEARM_TEST_STREAM` to any M4A URL to test the audio path without it.

## Lyrics

Tonearm asks these public lyric services in order, and stops at the first
one that has the track.

1. lrc.red, syllable timed
2. BetterLyrics, syllable timed
3. LRCLIB, line timed
4. YouTube Music's own lyrics, plain text

## Discord, Last.fm and Spotify

Open Settings, then Connections. Each service has its own card.

Release builds carry Tonearm's own Discord application and Last.fm keys,
so you only press Connect. Discord needs no sign in. Last.fm opens a page
in your browser where you approve Tonearm.

Spotify is optional, and needs a client ID and client secret of your own
from developer.spotify.com. With them, Tonearm looks each track up on
Spotify and reports Spotify's title, artist and album to Last.fm and
Discord. Without a sure match it reports the YouTube Music names. Canvas
clips also need a Spotify web sign in, which Connections explains.

A track scrobbles after it has played for half its length or four minutes,
as Last.fm asks.

## Your data

Everything stays on your computer.

* Settings, the sign in cookie and service keys are in
  `%APPDATA%\tonearm`, all encrypted with DPAPI.
* Offline songs, the song cache, yt-dlp and Canvas clips are in
  `%LOCALAPPDATA%\tonearm`.
* Listening history is kept encrypted on this computer, only to put what
  you play first in search. Settings can clear it.

Tonearm talks to YouTube Music, the lyric services above, GitHub (for
updates and yt-dlp), and Discord, Last.fm and Spotify only when you connect
them. Uninstalling keeps your data, in case you come back. Delete the two
folders above to remove it.

## Build

You need Windows and a recent stable Rust.

```bash
cargo build --release
```

The binary lands in `target/release/tonearm.exe`. For quicker optimised
builds while working, `cargo build --profile fast` skips link time
optimisation.

To build the installer as well, install [NSIS](https://nsis.sourceforge.io)
and run this from the repository root.

```bash
makensis -DVERSION=0.6.0 installer/tonearm.nsi
```

It writes `tonearm-setup.exe` next to `Cargo.toml`.

### Service keys

Tonearm's own Discord and Last.fm keys are not in this repository. The
build reads them from the environment, or from a `keys.env` file next to
`Cargo.toml` that git ignores:

```
TONEARM_DISCORD_APP=your application ID from discord.com/developers
TONEARM_LASTFM_KEY=your API key from last.fm/api/account/create
TONEARM_LASTFM_SECRET=the shared secret that comes with it
```

A build without them works too. Connections then asks for your own keys.

### Releases

Open the Actions tab, pick Release, press "Run workflow" and enter a
version such as `v0.6.0`. Pushing a version tag does the same. GitHub
builds the app and its installer on Windows and attaches both to a new
release. The workflow reads the three keys above from the repository
secrets.

## Layout

| Path | What it holds |
| --- | --- |
| `ui/app.slint` | The whole interface |
| `src/main.rs` | App state and wiring |
| `src/innertube/` | YouTube Music API client and parsers |
| `src/audio.rs`, `src/decode.rs` | Streaming download, decode and output |
| `src/resolver.rs` | Turns a song into an audio stream, with yt-dlp |
| `src/lyrics.rs`, `src/blur.rs` | Lyric services, and lyric lines drawn out of focus |
| `src/services.rs` | Discord, Last.fm and Spotify |
| `src/canvas.rs`, `src/motion.rs` | Spotify Canvas clips and animated covers |
| `src/offline.rs` | Offline songs and the song cache |
| `src/history.rs`, `src/session.rs` | Listening history, and where you left off |
| `src/media_keys.rs`, `src/taskbar.rs`, `src/shell.rs` | Media keys, taskbar buttons, tray and Start menu shortcut |
| `src/update.rs` | In-app updates |
| `src/store.rs` | Encrypted storage for settings and secrets |
| `installer/tonearm.nsi` | The NSIS installer |

## Contributing

Bug reports and pull requests are welcome. For anything large, please open
an issue first so we can talk it through.

## Licence

Tonearm is free software under the GNU General Public License, version 3.
See [LICENSE](LICENSE).

Tonearm uses Slint under its GPLv3 licence. The icons are from
[Phosphor](https://phosphoricons.com), under the MIT licence in
`ui/icons-LICENSE.txt`.
