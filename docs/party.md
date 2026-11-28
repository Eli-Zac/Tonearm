# Party

Plan for Tonearm's own version of a group listening session. A host starts a
**Party**, others join it by link or QR code, and everyone in it can add songs
to one shared queue that the host's device plays.

The feature is always called **Party** in the apps, the docs, the code and
commit messages. It is separate from joining a Spotify Jam
(`crates/tonearm-core/src/spotify_jam.rs`), which stays its own thing.

## What it should do

- **Start:** the host starts a Party from the queue (desktop or phone) or from
  Android Auto's Now playing. The Party gets a name ("Eli's Party"), a short
  code and a link.
- **Invite:** the join screen shows a QR code and a Copy link button. The QR
  holds the link, so a phone's own camera app can scan it and open the
  Tonearm app straight into the Party.
- **Join:** a guest opens the link (camera, a message, or pasted into the
  app), sees the Party's name, host and what is playing, and taps Join.
- **Share the queue:** everyone in the Party can add songs (Add to queue, Play
  next). Each queued song shows who added it.
- **Change what's playing:** with "Let others change what's playing" on (the
  host's switch, as Spotify has it), guests can also play, pause, skip and
  reorder. Off, they can only add songs.
- **Members:** a list of everyone in the Party, host first. The host can
  remove a member.
- **Leave and end:** a guest can leave. The host can end the Party, which
  ends it for everyone. A Party also ends when the host has been gone for a
  while (see "The server").

The host's device is the one that plays. Guests are remotes for it, the same
way a Tonearm Connect remote works (see `docs/tonearm-connect.md`).

## What exists already

- **Account server** (`server/`): a Cloudflare Worker with D1 at
  `api.tonearm.eli-zac.dev`. It has requests and answers only, with nothing
  pushed to a device yet. The Tonearm Connect plan already designs the first
  live piece (a Durable Object hub with WebSockets). A Party needs the same
  kind of piece, so whichever is built first sets the pattern for the other.
- **QR codes:** core `account::pair_qr` turns a pairing link into rows of
  modules, and both apps already draw it on their Pair a device pages. The
  phone can scan with Google's code scanner, which needs no camera permission
  (`account::pair_code_from_scan`).
- **Queue:** core `queue::Queue` on Android, and the desktop's own queue in
  `main.rs`. Both have Add to queue and Play next.
- **Android Auto:** `CarControls.kt` puts shuffle, repeat and like beside the
  transport through Media3 `CommandButton`s, and `AutoLibrary.kt` builds the
  browse tree the car shows.
- **Queue screens:** desktop `ui/app.slint` (the Queue panel, `QueueRow`),
  phone `QueueView.kt`.

## How it works

### Links and codes

- A Party has a code of 6 letters and digits without look-alikes (no 0/O,
  1/I/L), shown as `K7Q-M4P`.
- The link is `https://tonearm.eli-zac.dev/party/K7Q-M4P`. The QR code holds
  that link.
- **On a phone with Tonearm installed,** the link opens the app through an
  Android App Link. The site serves `/.well-known/assetlinks.json` naming the
  app's package and its signing certificate's SHA-256, and the manifest gets
  an `autoVerify` intent filter for `https://tonearm.eli-zac.dev/party/*`.
  The camera app then shows "Open in Tonearm". `assetlinks.json` lists both
  signing certificates, so release and debug builds both open links:
  - release (CI, `CN=Tonearm, O=Eli-Zac`):
    `90:BA:81:04:25:B4:87:00:25:D4:04:9A:97:D3:5E:07:F2:C0:F6:A4:2B:C0:4B:E2:BB:5D:DA:78:ED:78:EE:44`
  - debug (this PC's `~/.android/debug.keystore`):
    `1F:E9:35:DF:8C:7C:D3:9C:E2:C3:5E:18:3D:41:78:30:D2:0D:55:67:A6:11:02:B8:3A:7C:03:8C:4E:A9:F6:98`
- **On a phone without Tonearm,** the same link opens a small web page with the
  Party's name and what is playing, and a link to download the APK from the
  GitHub releases page.
- **On the desktop,** the installer registers `tonearm://` for the current user
  (in HKCU, so no admin rights). The web page has an "Open in Tonearm" button
  that opens `tonearm://party/K7Q-M4P`. Pasting the link or the code into the
  app's Join box works everywhere.
- The phone's in-app scanner also accepts party links, next to pairing links.

### The server

One Durable Object per Party, `PartyRoom`, found by `idFromName(code)`. It
holds the Party's live WebSockets, using the hibernation API, as the Tonearm
Connect hub does.

Routes:

| route | what it does |
| --- | --- |
| `POST /v1/party` | start a Party (signed in). Returns the code, link and a host token |
| `GET /v1/party/{code}` | public summary: name, host, now playing, member count. Used by the join screen and the web page |
| `POST /v1/party/{code}/join` | join with a display name. Returns a member token |
| `GET /v1/party/{code}/live` | WebSocket upgrade, with the host or member token |
| `DELETE /v1/party/{code}` | end it (host only) |
| `GET /party/{code}` | the web page for people without the app |
| `GET /.well-known/assetlinks.json` | the App Link proof for Android |

Messages over the socket are JSON objects with a `type`, as in Tonearm
Connect.

From the host: `state` (the song, playing or paused, position and when it
was measured, the queue with who added each song, the settings).

From a guest: `add {item, next}`, and when allowed `play`, `pause`, `next`,
`previous`, `seek`, `move {from, to}`, `play_item {item}`.

From the room: `state` (passed on to the guests), `members` (on every join
and leave), `command` (a guest's request, passed to the host), `removed` (to
a member the host removed), `ended`.

The room keeps only the latest state and the members. Nothing about what was
played is stored after the Party ends. If the host's socket is gone for 10
minutes the room tells everyone `ended` and deletes itself. A Party ends
after 12 hours whatever happens.

### The host's device

- Carries out guests' commands through the same functions its own buttons
  call. Adds land after the songs other people added, before the rest of
  the queue, as Spotify does it.
- Sends `state` when something changes, and every 15 seconds while playing.
- Remembers who added each song, so the queue can show it.
- Only the host scrobbles and records history. Guests do neither for songs
  they did not play themselves.

### A guest's device

- While in a Party, the player bar and Now playing show the Party's song and
  progress, with "In <host>'s Party" under it, as casting shows "Playing on".
- Add to queue and Play next anywhere in the app go to the Party.
- Pressing a song to play it asks "Add to the Party's queue?" when the host
  doesn't let others change what's playing.
- Leaving the Party gives the device back its own queue, as it was.

### Who can join

- The host needs a Tonearm account (starting a Party is a signed-in request).
- Guests do not. Joining asks for a name, filled in from the Tonearm account
  or the YouTube Music profile when there is one. The member token from
  `join` lasts for that Party only.
- Anyone with the link can join while the Party lasts. The host can remove
  people, and can make a new link (a new code) if one has spread too far.

## The apps

### Desktop and phone: the queue

At the top of the Queue panel (desktop) and the queue sheet (phone), above
Now playing, as in Spotify's Jam. On the phone the queue is a full-screen
sheet over Now playing (same flowing backdrop, a slim player along the
bottom), and the Party header, Now playing and Next up are one scrolling
list, so the header scrolls away and the songs get the screen.

**No Party:** a "Start a Party" row with a short line under it ("Listen
together. Friends add songs from their phones."), and a "Join a Party" button
that opens the Join box (paste a link or code; on the phone also Scan).

**In a Party (host):**

```
(avatars) Eli's Party          [Invite]  [⋯]
          3 people                        ├ Members
                                          └ End the Party

┌ Invite friends to your Party ──────────── × ┐
│ Copy and share the link, or ask them to     │
│ scan the QR code.                           │
│                                             │
│   [Copy link]                ┌─────────┐    │
│                              │   QR    │    │
│   K7Q-M4P                    └─────────┘    │
└─────────────────────────────────────────────┘
Let others change what's playing          (o)   one slim row, small switch

Now playing
...
Next from: Liked Songs   (rows show the adder's avatar)
```

- The invite card shows once when the Party starts and after that only when
  Invite is pressed. Its × closes it.
- The avatars are the first few members. Tapping them or the name, or
  Members in ⋯, opens the member list (name, avatar, "Host", the host can
  remove).
- Queue rows show the adder's avatar only for songs a member added, never
  the radio's, autoplay's or the host's own.

**In a Party (guest):** the same row with the Party's name and what the host
plays under it, Invite (so guests can pass it on), and ⋯ for Members and
Leave the Party. No switch.

Everything that loads (avatars, the QR) keeps its final size from the start,
with a skeleton in its place, so nothing in the queue moves.

### Android Auto

Spotify shows its Jam QR full screen because its car app runs on the Car App
Library's media templates. Tonearm can't do that: Android Auto only runs
Car App Library apps installed from a trusted store such as Google Play, and
its "Unknown sources" developer setting doesn't cover them. Tonearm installs
from GitHub, so it stays a plain media app (a Media3 session with a browse
tree), which that setting does cover.

Built as a prototype with a pretend Party (`PartyDemo.kt`) and tried on the
Desktop Head Unit against a real phone:

- **The button:** on Now playing, right of repeat (`CarControls.kt`). An
  outline people icon, "Start a Party"; a filled one while a Party runs,
  "View Party". A button can only send the app a command, not open a page,
  so pressing it shows a short note on the car's screen: "Party started.
  It's at the top of Home." (a `SessionError` with `INFO_CANCELLED`, which
  Android Auto shows as a banner, not a failure).
- **On Home:** while a Party runs, a row titled with the Party's name sits at
  the top of the Home tab, and goes when it ends. It can't be a tab of its
  own: Android Auto reads the tabs once per connection and shows four at
  most, and Tonearm has four. The row has three tiles:
  - **Scan to join:** the QR code as the tile's picture (black on white with a
    four-module quiet border, drawn by `PartyArt.kt`), the code under it.
    Opening it lists the code and the link as text.
  - **Members:** everyone in the Party with a letter avatar, "Host" or "Added
    3 songs". A guest opens a page with **Remove**.
  - **End Party.**
- **Actions:** End Party and Remove are playable items. Picking one does the
  action and hands the car back the queue as it is (`TonearmPlayer.keepQueue`),
  so the music carries on without a break (checked: same song, still
  playing, position moving on). Android Auto then shows Now playing.
- **Live updates:** Home and Members change in place as the Party does,
  through `notifyChildrenChanged`. This needed `onSubscribe` in
  `PlaybackService`: Media3's default looked each page up with `onGetItem`,
  which Tonearm doesn't answer, so every subscription was refused and no
  change ever reached the car.

Known rough edges: a removed member's own page shows "No items" until the
car goes back; and the QR tile's size is the car's choice (about a quarter
of the screen's width on the head unit), still to be tried with a phone
camera in a real car.

## Security and limits

- Codes come from a cryptographic random source. 6 characters from 31 gives
  about 887 million codes, and joining is rate limited per address.
- Member tokens are only good for one Party, and a removed member's token
  stops working at once.
- A guest can never see the host's account, devices or history. A guest
  only sees the Party's songs and the members' names and avatars.
- Messages over 64 KB are refused, and each member is limited in how many
  messages they can send per second.
- Durable Objects with SQLite storage are on Cloudflare's free plan.
  Hibernation keeps idle Parties close to free.

## Phases

Each phase works on its own and is tested before the next starts.

1. **Server and core client.** `PartyRoom`, the routes, the web page and
   `assetlinks.json`. Core `party.rs`: start, look up, join, the socket thread
   (reconnecting with backoff), message types with tests, `party_qr`, and the
   link and code parsing. Tried with a command-line client in the core's
   examples: start, join from a second profile, add, members, end, and the
   host disappearing.
2. **Desktop.** The Party header in the Queue panel, invite card, members,
   host and guest modes, `tonearm://` in the installer. Tried with two
   desktop copies under their own profiles.
3. **Phone.** The same in the queue sheet, the App Link, Scan in the Join box.
   Tried phone and desktop together, and scanning the desktop's QR with the
   phone's own camera app.
4. **The Party in the car.** Built ahead as a prototype on a pretend Party
   (see "Android Auto"). What's left is swapping `PartyDemo` for the real
   Party, then trying it in the car, scanning the screen with a phone.
5. **Later, if wanted.** Listen along: each guest's own device plays the
   Party's song in step with the host, for Parties where people aren't in the
   same room.

Rough size: phase 1 about one session, phases 2 and 3 about one each, phase
4 a little, plus testing on real devices.

## Decided

- The host's device plays and guests are remotes. Listening along on your
  own device comes later (phase 5).
- Android Auto stays a plain media app (templates need a Play install), with
  the Party as a row at the top of Home.
- Guests need the Tonearm app but not a Tonearm account. Only the host signs
  in.
- Party is built first. Its live server piece (a Durable Object with
  hibernating WebSockets) sets the pattern that Tonearm Connect's hub reuses.
