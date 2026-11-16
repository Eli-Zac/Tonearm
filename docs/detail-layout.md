# Tonearm detail page layout (artist / album / playlist / single / podcast / profile / mix)

Status: design only. No repo files were edited. Written against `ui/app.slint`
(AppWindow main island, `ListView` over `rows: [RowData]`), `src/innertube/{model,parse}.rs`
and `src/main.rs` (`open`, `show`, `append`, `item_data`, `place_thumb`, `set_tint`).
Anything marked **(guess)** is inferred from memory of YouTube Music's JSON and must be
checked against a real response (dump one with `RUST_LOG`/a temp `eprintln!` of the response
before relying on it).

Facts taken from the code that the design depends on:

- Today the page title/subtitle/Play/Shuffle bar is a fixed 72px strip *above* the `ListView`
  (shown when `page-title != ""`). It does not scroll.
- `ListView` rows are `Rectangle`s with a height chosen from `row.kind` (0 header 56, 1 item 60,
  2 carousel 236, 3 load-more 120). Rows use `width: parent.width - 10px`; the enclosing
  `VerticalLayout` has `padding-left: 16px; padding-right: 6px`.
- `main-width = root.width - side(216) - gap*3 - (panel open ? panel-width + gap : 0)`. With the
  820px min window the island is about 580px wide, less when the queue/lyrics panel is open.
- `images::fetch` always **centre-crops to a square** of `px`. A wide artist banner therefore
  needs a new fetch path (section 2.4).
- `Page::thumb` is already used to back-fill track art on albums (`parse::page`), and `append`
  fetches a 64px thumb for every list row. Numbered album rows do not draw art, so skip that
  fetch for them.
- `tint` + `set_tint` (main.rs) averages a cover and pulls it to peak 150. The window animates
  it as `shown-tint` over 700ms. The hero reuses exactly this idea with a second property.

---------------------------------------------------------------------------------------------

## 1. Visual spec

Common frame. The hero is an **inset rounded card** (radius `Theme.radius` 14px) inside the
existing 16px/6px list padding, not an edge-to-edge bleed. Reason: the `ListView` clips to its
own viewport and sits in a padded layout, so a full-bleed gradient would need negative x.
The inset card avoids that and looks like the existing `Section` cards.

Background of the hero: vertical gradient from `page-tint` at 75% alpha at the top to 12% alpha
at the bottom, over `Theme.island`. Text sits on it, so keep the tint at peak 120-150 (readable
white text). Wide = main-width >= 700px, compact = below it.

Sizes (wide / compact):

| thing | wide | compact |
|---|---|---|
| hero padding | 24 | 16 |
| art (square) | 216 | 140 |
| art to text gap | 24 | 16 |
| title font | 48 (34 when > 26 chars) | 30 (24 when > 26 chars) |
| hero height (square art) | 288 | 232 |
| hero height (artist banner) | 300 | 220 |
| hero height (no art, generic) | 168 | 140 |
| gap under hero (part of row) | 12 | 12 |

Title is one line, `overflow: elide`, weight 800, so the hero height never depends on text
(rows in a `ListView` need a height that does not depend on layout feedback).

### 1.1 Album (`kind 2`), wide

```
 16px                                                                  6px
  +------------------------------------------------------------------+
  |  (tint gradient, radius 14)                                      |
  |  24                                                              |
  |  +----------------+                                              |
  |  |                |  Album                       <- 12px 700 dim |
  |  |   216 x 216    |  Discovery                   <- 48px 800     |
  |  |   square art   |  Daft Punk                   <- 14px 700, clickable (opens artist)
  |  |   radius 10    |  2001  •  14 songs  •  1 hr 1 min  <- 13px dim
  |  |   drop shadow  |                                              |
  |  |                |  [> Play] [Shuffle] [heart]   <- 36px row, aligned to art bottom
  |  +----------------+                                              |
  |  24                                                              |
  +------------------------------------------------------------------+
   12px gap
   #   Title                                           Album*     time   <- kind 6 column header, 32px, dim 12px, bottom 1px Theme.line
   1   One More Time                                              5:20   <- numbered row, 52px, NO art
   2   Aerodynamic                                                3:32
   ...
   About (kind 5 text block, only if description exists, 3 lines)
   More by Daft Punk  (kind 0 header)  [carousel of albums]   <- albums pages often carry "Other versions"/"More by"
```

*Album column is not shown on album pages (all tracks share the album). The column header there
is `#  Title  ...  time`.

Row details for numbered rows (SongRow with `number > 0`):
- number column 36px wide, centred, 13px dim, tabular enough with Inter.
- hover: number replaced by a play glyph (`Icons.play`, 14px, `Theme.text`), row background
  `Theme.hover` (existing behaviour).
- current track: number and title in `Theme.accent` (existing `current` colouring).
- title 14px/500, subtitle 12px dim only when the track artist differs from the album artist
  (Rust blanks it otherwise, otherwise every row repeats the artist). Duration right aligned.
- no per-row art (saves the thumb fetches too).

### 1.2 Artist (`kind 1`), wide, with banner

```
  +------------------------------------------------------------------+
  | [full-width banner image, cover-cropped, 300px tall]             |
  | [bottom 60% black gradient #00000000 -> #000000cc]               |
  |                                                                  |
  |                                                                  |
  |  Artist                                    <- label, 12px, #ffffffcc
  |  Daft Punk                                 <- 48px 800, white
  |  12.3M subscribers  •  8.1M monthly listeners   <- 13px, #ffffffcc
  |  [> Play] [Shuffle] [Follow]  [...]        <- Follow is a pill, "Following" when subscribed
  +------------------------------------------------------------------+
   Popular                                     [Show all]   <- kind 0 with more=true
   1  [no art] One More Time   (plays col)           5:20      <- numbered rows, max 5
   ...
   Albums                                      [Show all]
   [card][card][card][card] ->                  <- kind 2 carousel, square cards
   Singles & EPs / Videos / Featured on / Fans might also like (round cards)
   About   (kind 5, description)
```

No banner (`banner == ""` or fetch failed): same layout as album but art is **round**
(`round: true`, 216 / 140px) and the label is "Artist". Hero height is the square one.

Rules specific to artists:
- Label is "Artist"; byline is empty; meta line is subscribers (+ monthly listeners if present).
- Follow replaces Like. Heart is not shown.
- "Songs" shelf from InnerTube is renamed "Popular" in Rust; capped at 5 rows; "Show all"
  appears when the shelf has a browse endpoint (section 2.3). The numbers are 1..5.
  Artist top songs rows show album art? Spotify does. Here: **no art, numbered**, same as album,
  to keep one row look. (If you prefer art, set `style: 3` = numbered + art; the row code below
  supports `number > 0 && show-art`.)
- "Fans might also like" carousel uses `round` cards (already handled: `item.round` for
  `Kind::Artist` in `item_data`).

### 1.3 Playlist (`kind 3`) and Mix

```
  +------------------------------------------------------------------+
  |  +--------------+  Playlist                                       |
  |  |  216 square  |  Late Night Drive                               |
  |  |   art        |  Eli                         <- owner (byline), clickable if it has a browse id
  |  +--------------+  2024  •  50 songs  •  3 hr 12 min              |
  |                    [> Play] [Shuffle] [heart]                     |
  +------------------------------------------------------------------+
   #   Title                         Album                    time    <- kind 6 header; Album hidden when compact
   1  [44 art]  Song title           Album name               3:20    <- numbered + art, 60px
              Artist
```

- Rows: number column 36px AND 44px art (style 2). Number column is 28px here to save width.
- Album column: `show-album: main-width >= 760px`. It is a fixed `root.width * 0.26` text, elided,
  dim. Clicking the album name is out of scope (the whole row plays).
- Auto-generated YouTube playlists / the user's own playlists: label "Playlist"; mixes
  (`RDCLAK...`, no browse page; they are queued via `open_item`'s `playlist_id` branch) never
  reach this page. If a mix does render as a page, label "Mix" and hide Like.
- Playlists with continuation (`kind 3` load-more row) already work; the hero is only emitted
  with the first page.

### 1.4 Single / EP (`kind 4`)

Identical to album, label is "Single" or "EP" (from the subtitle's first run), and since they
have 1-4 tracks the **column header row is omitted** and a trailing "More by <artist>" carousel
(if the page has one) follows. Row style numbered.

### 1.5 Podcast / audiobook (`kind 5`, "Podcast") and user profile (`kind 6`)

- Podcast show: square art, label "Podcast", byline = author, meta = episode count. Rows are
  episodes: style 0 (art + title + description subtitle + duration), no numbers, Play button
  plays latest episode (`play-page(false)`), no Shuffle (`can-shuffle: false`), Follow shown if
  the subscribe button exists.
- User profile (`MUSIC_PAGE_TYPE_USER_CHANNEL`): round art 140/216, label "Profile", no
  Play/Shuffle (`can-play: false`); body is just carousels.

### 1.6 Generic fallback (`kind 0` header, or any other browse page)

Anything without a recognised header (Home, Explore, Library, Search, charts): keep the
**existing** title/subtitle strip and behaviour unchanged. A browse page whose header is
recognised but has no art (rare): hero with no art, text at x = pad, height 168/140, no
gradient art, buttons per flags.

---------------------------------------------------------------------------------------------

## 2. Data model

### 2.1 Slint

```slint
// kind: 0 none (old title strip), 1 artist, 2 album, 3 playlist, 4 single or EP,
//       5 podcast, 6 profile, 7 other
export struct PageHeader {
    kind: int,
    label: string,
    title: string,
    byline: string,
    byline-browsable: bool,
    meta: string,
    art: image,
    has-art: bool,
    round: bool,
    // The art is a wide banner, drawn across the whole hero.
    banner: bool,
    can-play: bool,
    can-shuffle: bool,
    can-follow: bool,
    followed: bool,
    can-like: bool,
    liked: bool,
}
```

`ItemData` gains one field (needed for the playlist album column):

```slint
export struct ItemData {
    id: int,
    title: string,
    subtitle: string,
    album: string,      // new, empty when unknown
    duration: string,
    art: image,
    has-art: bool,
    round: bool,
}
```

`RowData` (replace the comment and add fields; existing kinds 0-3 keep their meaning, so
existing Rust that builds `RowData { kind: 0, title, ..default }` still compiles):

```slint
// kind: 0 section header, 1 list item, 2 row of cards, 3 load more marker,
//       4 page hero (drawn from AppWindow.header), 5 text block (title holds the text),
//       6 column header for a track list
// style (kind 1 only): 0 art, 1 numbered no art, 2 numbered with art
export struct RowData {
    kind: int,
    title: string,
    item: ItemData,
    cards: [ItemData],
    style: int,
    number: int,     // 1-based track number, kind 1 style 1/2
    more: bool,      // kind 0: show a "Show all" button
    more-id: int,    // kind 0: index into PageState.mores
}
```

AppWindow additions:

```slint
in property <PageHeader> header;
// Colour pulled from the page art. Tints the hero like `tint` tints the player.
in property <color> page-tint: #2b2440;
property <color> shown-page-tint: page-tint;
animate shown-page-tint { duration: 700ms; easing: ease-in-out; }
callback show-all(int);
callback byline-clicked();
callback toggle-follow();
callback toggle-like-page();
// play-page(bool) already exists and is reused for Play (false) and Shuffle (true).
property <bool> compact: main-width < 700px;
```

`page-title`/`page-subtitle`/`page-playable` stay for non-detail pages. The old strip becomes
`if root.nav != 3 && root.header.kind == 0 && root.page-title != "": ...`. The Play/Shuffle
visibility on the old strip is unchanged. Detail pages use `header.can-play`.

### 2.2 Rust model (`src/innertube/model.rs`)

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageKind { Artist, Album, Single, Playlist, Podcast, Profile, #[default] Other }

#[derive(Clone, Debug, Default)]
pub struct Header {
    pub kind: PageKind,
    pub label: String,            // "Album", "EP", "Playlist"...
    pub title: String,
    pub byline: String,           // artist or owner
    pub byline_browse_id: Option<String>,
    pub meta: String,             // "2001 • 14 songs • 1 hr 1 min", or subscribers
    pub about: String,            // description, may be empty
    pub thumb: String,            // square art or artist banner
    pub banner: bool,             // thumb is a wide banner (immersive header)
    pub subscribed: Option<bool>, // Some when a subscribe button exists
    pub play: Option<Item>,       // artist playButton watchEndpoint, for Play on artist pages
}

// Page: add
pub header: Option<Header>,
// Shelf: add
pub more: Option<(String, Option<String>)>,   // (browse_id, params) behind "Show all"
// Item: add
pub album: String,
```

`Page.title/subtitle/thumb` stay filled (back-compat, Home etc., and the thumb back-fill in
`parse::page`). `header` is `Some` only for `musicResponsiveHeaderRenderer`,
`musicImmersiveHeaderRenderer`, `musicDetailHeaderRenderer` (and the playlist-editable wrapper).

### 2.3 What to parse, and from where

Header renderer keys (inside `Walker::header`, branch on which key matched; pass the key name in,
the walker currently discards it: change `k if HEADERS.contains(&k) => self.header(child)` to
`self.header(k, child)`).

`musicResponsiveHeaderRenderer` (albums and playlists on the current web client):
- `title` -> title (already read).
- `subtitle.runs`: split on runs equal to `" • "`. Run 0 text is the type label ("Album",
  "Single", "EP", "Playlist", "Podcast") and last run is usually the year **(guess)**.
- `straplineTextOne.runs`: the artist/owner. Run with
  `navigationEndpoint.browseEndpoint.browseId` gives `byline_browse_id` (starts `UC` for artists;
  `MPREb_`/`VL` not used here).
- `secondSubtitle.runs`: "14 songs", "1 hour, 1 minute" separated by " • " **(guess on exact
  wording; just join with " • " as today)**.
- `description.musicDescriptionShelfRenderer.description.runs` -> about **(guess on nesting;
  the walker's `find(h, "description")` is a safe lookup)**.
- `thumbnail.musicThumbnailRenderer.thumbnail.thumbnails` (existing `thumb()` already searches
  `thumbnails` depth-first). Existing `thumb()` returns the first >= 120px; for the hero choose
  the **largest**: add `fn thumb_best(v) -> String` taking `list.last()`.
- `buttons[]`: contains `musicPlayButtonRenderer` (play) and `toggleButtonRenderer`
  (library add) and `menuRenderer` **(guess)**. Not needed for v1: Play/Shuffle go through the
  existing `page_songs()` queue path.
- Playlist pages wrap this in `musicEditablePlaylistDetailHeaderRenderer.header
  .musicResponsiveHeaderRenderer` for playlists you own **(guess)**. The recursive walker already
  finds the inner renderer, so only add the wrapper name to `HEADERS` if the wrapper itself ends
  up swallowing it (check with a dump).

`musicDetailHeaderRenderer` (older albums/playlists, still served for some):
- `title`, `subtitle.runs` = ["Album", " • ", "Daft Punk", " • ", "2001"] (artist run carries the
  browseEndpoint), `secondSubtitle` = "14 songs • 1 hour, 1 minute",
  `description`, `thumbnail.croppedSquareThumbnailRenderer.thumbnail.thumbnails`.

`musicImmersiveHeaderRenderer` (artist):
- `title`, `description` (about).
- `thumbnail.musicThumbnailRenderer.thumbnail.thumbnails`: the wide banner (set `banner: true`).
- `subscriptionButton.subscribeButtonRenderer`: `subscriberCountText` ("12.3M subscribers"),
  `subscribed` (bool), `channelId` **(known)**. Subscribed state needs a login; unauthenticated
  it is still false.
- `monthlyListenerCount` (runs/simple text) at the top level of the renderer **(guess; present
  on newer responses)**.
- `playButton.buttonRenderer.navigationEndpoint.watchEndpoint` (videoId + playlistId) and
  `startRadioButton.buttonRenderer.navigationEndpoint.watchEndpoint` **(guess names)**; store
  the first as `Header.play` so artist Play starts the artist's radio. Fallback: `page_songs()`.

Type from the data, in priority order:
1. immersive header -> Artist.
2. label text: "Album" -> Album, "Single"/"EP" -> Single, "Playlist"/"Mix" -> Playlist,
   "Podcast"/"Show" -> Podcast, "Profile"/"User" -> Profile **(guess on exact strings)**.
3. The caller's `Route::Browse.id` prefix: `UC` artist, `MPREb_` album, `VL` playlist,
   `MPSP` podcast **(guess)**. This is also what picks the skeleton before data arrives.

Shelf extras:
- Artist "Songs" shelf: `musicShelfRenderer.bottomEndpoint.browseEndpoint.{browseId,params}`
  is the "Show all" target **(guess; fall back to `title.runs[0].navigationEndpoint`)**.
- Carousel: `header.musicCarouselShelfBasicHeaderRenderer.moreContentButton.buttonRenderer
  .navigationEndpoint.browseEndpoint.{browseId,params}` **(guess, widely seen)**. Add to
  `Walker::shelf`.
- On an artist page, rename the shelf titled "Songs" to "Popular" and truncate to 5 items when
  `more.is_some()`.

Item extras:
- `Item.album`: in `list_item`, pick the flex column whose text run has
  `navigationEndpoint.browseEndpoint...pageType == "MUSIC_PAGE_TYPE_ALBUM"`; drop it from the
  joined subtitle so the album does not appear twice. Playlist rows are
  [title, artist, album] columns; album tracks are [title, (artist?), plays] **(guess)**.
- Album rows: when a track's artist equals `Header.byline` blank the subtitle (done in Rust at
  row build time, not in the parser).

Test to add in `parse.rs`: a JSON with a `musicResponsiveHeaderRenderer` (runs as above) asserting
`header.kind == Album`, `byline`, `meta`, and one with `musicImmersiveHeaderRenderer` asserting
`banner` and `subscribed`.

### 2.4 Rust app state (`src/main.rs`)

- `PageState`: add `mores: Vec<(String, Option<String>)>`, `header: Option<Header>`,
  `hero_art: bool`. `open()` resets via `..PageState::default()` already.
- `Target`: add `Hero { generation: u64 }`. In `place_thumb`:
  ```rust
  Target::Hero { generation } => {
      if self.page.borrow().generation == generation {
          let mut h = self.ui.get_header();
          h.art = image.clone(); h.has_art = true;
          self.ui.set_header(h);
          if let Some(c) = tint_of(&image, 120.0) { self.ui.set_page_tint(c); }
      }
  }
  ```
  Refactor `set_tint` into `fn tint_of(image:&Image, peak:f32)->Option<slint::Color>` (the loop
  body already exists) plus a thin `set_tint` wrapper (player uses 150, hero 120).
- `images`: add `fetch_wide(http, url, w, h)` that fetches `sized`-style `=w{w}-h{h}-l90-rj`
  **(guess: the googleusercontent suffix honours independent w/h)**, centre-crops to w:h (a 4:1
  crop of 1200x300 for the hero), no rounding. `Target::Hero` calls it when `header.banner`,
  else the normal `fetch` at 440px (2x of the 216px display) with `round` for artist profile
  art. Cache key must include w/h: change `Pixels` key to `format!("{w}x{h}{}{url}", ...)`.
- `open()`: in addition to the existing resets call `self.ui.set_header(PageHeader::default())`
  and, from the route id, set only `kind` and `banner` guess so the skeleton matches:
  `set_header(PageHeader { kind: guess_kind(&route), banner: matches!(kind,1), ..default })`.
  `loading` is already true. When data arrives `show()` overwrites it.
- `show()`: if `page.header.is_some()` build `PageHeader` from `Header`, keep `art` from the
  existing `header` (do not blank an already loaded image on page 2), call `set_header`, skip the
  old `set_page_title` (leave title empty so the old strip is hidden by the
  `header.kind == 0` guard), kick off the hero thumb with `Target::Hero`.
- `append()` first page (`page.items.is_empty() && rows.row_count() == 0`): push
  `RowData { kind: 4, ..default }`, then for album/single/playlist push a `kind: 6` column header
  (skip for single/EP, profile, podcast, artist). For each shelf:
  - titled shelf -> `RowData { kind: 0, title, more: shelf.more.is_some(), more_id: mores.len() }`
    and `mores.push(..)`.
  - non-carousel items: `style` = 1 for album/single/artist-Popular, 2 for playlist, 0 otherwise;
    `number` = 1-based index within the shelf for styles 1/2; for style 1 call
    `item_data(..., px = 0)` meaning "do not fetch" (add `if px == 0 { None }` to `thumb()` caller)
    so no 64px art is requested.
  - `Header.about` non-empty -> push `kind: 5` row at the end of the first page (title = text).
  - `place` bookkeeping: `Place::Row(rows.row_count())` stays correct because the hero and
    header rows are pushed before it.
- `playable` (page_songs) unchanged; set `header.can_play = header.can_shuffle = playable` after
  `append` (replace `self.ui.set_page_playable(playable)` with a helper that also patches
  `header`). `play_page(shuffle)` stays as is; for an artist with `Header.play` and
  `!shuffle`, call `open_item(play.clone())` instead.
- New callbacks wired in `main()`: `show_all(id)` -> `open(Route::Browse{ id, params }, true)`
  from `page.mores[id]`; `byline_clicked` -> open `Route::Browse` with
  `header.byline_browse_id`. `toggle_follow` / `toggle_like_page` need subscribe/like
  endpoints that do not exist yet: **v1 hides both** (`can_follow = false`, `can_like = false`)
  and the buttons ship dormant; enable per the plan in section 5 when the client gets
  `subscribe`/`like/like` calls.

---------------------------------------------------------------------------------------------

## 3. Slint code

All code is for Slint 1.18 and written in the existing style: named `Theme` colours, `Art`,
`Icon`, `IconButton`, `PillButton`, `Bone`, absolute positions inside fixed-size rectangles,
layouts only where the parent has a fixed size (as `SongRow` does). Place `SkeletonDetail`
after `SkeletonPage`, and `SectionHeader`/`DetailHeader` after `Carousel` (they use `Art`,
`PillButton`, `IconButton`).

### 3.1 SongRow, extended (replaces the existing component; defaults keep old behaviour)

```slint
component SongRow inherits Rectangle {
    in property <ItemData> item;
    in property <bool> current;
    // 0 draws no number. Otherwise the 1-based track number sits before the title.
    in property <int> number;
    in property <bool> show-art: true;
    // The album name in a column before the duration (playlists on a wide window).
    in property <bool> show-album;
    callback activated;
    height: number > 0 && !show-art ? 52px : 60px;
    border-radius: 10px;
    background: current ? #ffffff14 : touch.has-hover ? Theme.hover : transparent;
    animate background { duration: 120ms; }
    touch := TouchArea {
        mouse-cursor: pointer;
        clicked => { root.activated(); }
    }
    HorizontalLayout {
        padding-left: 8px;
        padding-right: 14px;
        spacing: 12px;
        if root.number > 0: Rectangle {
            width: 28px;
            Text {
                text: touch.has-hover ? "" : root.number;
                color: root.current ? Theme.accent : Theme.dim;
                font-size: 13px;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
            if touch.has-hover: Icon {
                text: Icons.play;
                font-size: 14px;
            }
        }
        if root.show-art: VerticalLayout {
            alignment: center;
            Art {
                width: 44px;
                height: 44px;
                corner: 6px;
                source: item.art;
                has: item.has-art;
                round: item.round;
            }
        }
        VerticalLayout {
            alignment: center;
            horizontal-stretch: 1;
            spacing: 3px;
            Text {
                text: item.title;
                color: current ? Theme.accent : Theme.text;
                font-size: 14px;
                font-weight: 500;
                overflow: elide;
            }
            if item.subtitle != "": Text {
                text: item.subtitle;
                color: Theme.dim;
                font-size: 12px;
                overflow: elide;
            }
        }
        if root.show-album: Text {
            width: root.width * 0.26;
            text: item.album;
            color: Theme.dim;
            font-size: 12px;
            vertical-alignment: center;
            overflow: elide;
        }
        Text {
            text: item.duration;
            color: Theme.dim;
            font-size: 12px;
            vertical-alignment: center;
            horizontal-stretch: 0;
        }
    }
}
```

Note the title column is `VerticalLayout alignment: center`, so a one-line numbered album row
(no subtitle) centres the title in its 52px row.

### 3.2 SectionHeader (kind 0, with optional "Show all")

```slint
// The title above a shelf. `more` adds a quiet "Show all" button on the right.
component SectionHeader inherits Rectangle {
    in property <string> title;
    in property <bool> more;
    callback more-clicked;
    height: 56px;
    Text {
        x: 8px;
        y: 18px;
        height: 34px;
        width: parent.width - 16px - (root.more ? 96px : 0px);
        text: root.title;
        color: Theme.text;
        font-size: 20px;
        font-weight: 700;
        vertical-alignment: center;
        overflow: elide;
    }
    if root.more: Rectangle {
        x: parent.width - 92px;
        y: 22px;
        width: 88px;
        height: 28px;
        border-radius: 14px;
        background: all.has-hover ? Theme.hover : transparent;
        animate background { duration: 120ms; }
        Text {
            text: "Show all";
            color: all.has-hover ? Theme.text : Theme.dim;
            font-size: 13px;
            font-weight: 600;
            horizontal-alignment: center;
            vertical-alignment: center;
        }
        all := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.more-clicked(); }
        }
    }
}
```

### 3.3 TrackColumns (kind 6)

```slint
// "# Title    Album    time" above a track list.
component TrackColumns inherits Rectangle {
    in property <bool> numbered: true;
    in property <bool> show-album;
    height: 32px;
    Text {
        x: 8px + 12px;
        width: 28px;
        height: parent.height;
        text: "#";
        color: Theme.faint;
        font-size: 12px;
        font-weight: 600;
        horizontal-alignment: center;
        vertical-alignment: center;
    }
    Text {
        x: 8px + 28px + 12px;
        height: parent.height;
        text: "Title";
        color: Theme.faint;
        font-size: 12px;
        font-weight: 600;
        vertical-alignment: center;
    }
    if root.show-album: Text {
        x: parent.width - 14px - 56px - 12px - parent.width * 0.26;
        width: parent.width * 0.26;
        height: parent.height;
        text: "Album";
        color: Theme.faint;
        font-size: 12px;
        font-weight: 600;
        vertical-alignment: center;
    }
    Rectangle {
        y: parent.height - 1px;
        height: 1px;
        background: Theme.line;
    }
}
```

The `x` of the "Album" label assumes the 56px-wide duration cell. In `SongRow` the duration
Text is content-width, not 56px, so measure once and adjust: either give the duration `Text`
in `SongRow` a fixed `width: 44px; horizontal-alignment: right;` (recommended, also aligns
durations) and use `44px` here, or accept a few px of misalignment. Use the fixed width and
change the constant above to `14px + 44px + 12px`.

### 3.4 DetailHeader

```slint
component DetailHeader inherits Rectangle {
    in property <PageHeader> data;
    in property <color> tint: Theme.raised;
    in property <bool> compact;
    callback play();
    callback shuffle();
    callback follow();
    callback like();
    callback byline-clicked();
    property <length> pad: compact ? 16px : 24px;
    property <length> art-size: compact ? 140px : 216px;
    property <bool> banner: data.banner && data.has-art;
    property <bool> has-side-art: !banner && data.kind != 7;
    property <length> text-x: has-side-art ? pad + art-size + (compact ? 16px : 24px) : pad;
    // On the banner the text sits on the picture, so it is always light.
    property <color> soft: banner ? #ffffffcc : Theme.dim;
    border-radius: Theme.radius;
    clip: true;
    background: @linear-gradient(180deg, root.tint.with-alpha(0.75) 0%, root.tint.with-alpha(0.12) 100%);

    if root.banner: Image {
        width: 100%;
        height: 100%;
        source: root.data.art;
        image-fit: cover;
    }
    if root.banner: Rectangle {
        background: @linear-gradient(180deg, #00000000 25%, #000000d0 100%);
    }

    // The square or round art, with a soft shadow. The shadow lives on an outer
    // rectangle because the art itself clips to its rounded corners.
    if root.has-side-art: Rectangle {
        x: root.pad;
        y: root.pad;
        width: root.art-size;
        height: root.art-size;
        border-radius: root.data.round ? root.art-size / 2 : 10px;
        drop-shadow-blur: 28px;
        drop-shadow-color: #00000070;
        drop-shadow-offset-y: 8px;
        Art {
            width: 100%;
            height: 100%;
            clip: true;
            corner: 10px;
            source: root.data.art;
            has: root.data.has-art;
            round: root.data.round;
        }
    }

    // Text and buttons. Bottom aligned so the actions line up with the bottom of the art.
    Rectangle {
        x: root.text-x;
        y: root.has-side-art ? root.pad : 0px;
        width: root.width - root.text-x - root.pad;
        height: root.has-side-art ? root.art-size : root.height - root.pad;
        VerticalLayout {
            alignment: end;
            spacing: 6px;
            Text {
                text: root.data.label;
                color: root.soft;
                font-size: 12px;
                font-weight: 700;
                overflow: elide;
            }
            Text {
                text: root.data.title;
                color: #ffffff;
                font-size: root.data.title.character-count > 26 ? (root.compact ? 24px : 34px) : (root.compact ? 30px : 48px);
                font-weight: 800;
                overflow: elide;
            }
            if root.data.byline != "": HorizontalLayout {
                alignment: start;
                Rectangle {
                    height: 20px;
                    width: by.preferred-width;
                    by := Text {
                        text: root.data.byline;
                        color: by-touch.has-hover && root.data.byline-browsable ? Theme.accent : #ffffff;
                        font-size: 14px;
                        font-weight: 700;
                        vertical-alignment: center;
                    }
                    by-touch := TouchArea {
                        mouse-cursor: root.data.byline-browsable ? pointer : default;
                        clicked => {
                            if (root.data.byline-browsable) { root.byline-clicked(); }
                        }
                    }
                }
            }
            if root.data.meta != "": Text {
                text: root.data.meta;
                color: root.soft;
                font-size: 13px;
                overflow: elide;
            }
            HorizontalLayout {
                height: 36px + 10px;
                padding-top: 10px;
                spacing: 10px;
                alignment: start;
                if root.data.can-play: PillButton {
                    text: "Play";
                    primary: true;
                    clicked => { root.play(); }
                }
                if root.data.can-shuffle && !root.compact: PillButton {
                    text: "Shuffle";
                    clicked => { root.shuffle(); }
                }
                if root.data.can-shuffle && root.compact: Rectangle {
                    width: 36px;
                    height: 36px;
                    border-radius: 18px;
                    background: #ffffff14;
                    IconButton {
                        icon: Icons.shuffle;
                        clicked => { root.shuffle(); }
                    }
                }
                if root.data.can-follow: PillButton {
                    text: root.data.followed ? "Following" : "Follow";
                    clicked => { root.follow(); }
                }
                if root.data.can-like: Rectangle {
                    width: 36px;
                    height: 36px;
                    border-radius: 18px;
                    background: #ffffff14;
                    IconButton {
                        icon: Icons.heart;
                        tint: root.data.liked ? Theme.accent : Theme.text;
                        clicked => { root.like(); }
                    }
                }
            }
        }
    }
}
```

Check on first compile: `root.data.title.character-count` (string property, Slint >= 1.4)
and `drop-shadow-*` on a plain `Rectangle` are both in 1.18. If `clip: true` on `Art` is not
accepted because `Art` defines its own, set it on the inner `Rectangle` instead; it is a
`Rectangle` property so it should pass through.

The hero uses `#ffffff` for the title instead of `Theme.text` (`#f5f5f7`) deliberately: the
gradient is tinted. If you want to stay literal use `Theme.text`.

### 3.5 AppWindow wiring (replaces the old strip and the `ListView` body)

Row height as a pure function in AppWindow, so the rows have a height that never depends on
layout:

```slint
property <bool> compact: main-width < 700px;
property <length> hero-height: header.kind == 7 ? (compact ? 140px : 168px)
    : header.banner ? (compact ? 220px : 300px)
    : (compact ? 232px : 288px);

pure function row-height(kind: int, style: int) -> length {
    return kind == 0 ? 56px
        : kind == 1 ? (style == 1 ? 52px : 60px)
        : kind == 2 ? 236px
        : kind == 4 ? hero-height + 12px
        : kind == 5 ? 96px
        : kind == 6 ? 32px
        : 120px;
}
```

Old title strip: change its condition to
`if root.nav != 3 && root.header.kind == 0 && root.page-title != "": HorizontalLayout { ... }`
(the line `if root.page-title != "":` inside the page `VerticalLayout`).

`ListView` body:

```slint
ListView {
    vertical-stretch: 1;
    for row in root.rows: Rectangle {
        height: root.row-height(row.kind, row.style);
        if row.kind == 0: SectionHeader {
            width: parent.width - 10px;
            title: row.title;
            more: row.more;
            more-clicked => { root.show-all(row.more-id); }
        }
        if row.kind == 1: SongRow {
            width: parent.width - 10px;
            item: row.item;
            number: row.number;
            show-art: row.style != 1;
            show-album: row.style == 2 && !root.compact && root.main-width >= 760px;
            activated => { root.item-activated(row.item.id); }
        }
        if row.kind == 2: Carousel { /* unchanged */ }
        if row.kind == 3: Appear { /* unchanged */ }
        if row.kind == 4: DetailHeader {
            width: parent.width - 10px;
            height: parent.height - 12px;
            data: root.header;
            tint: root.shown-page-tint;
            compact: root.compact;
            play => { root.play-page(false); }
            shuffle => { root.play-page(true); }
            follow => { root.toggle-follow(); }
            like => { root.toggle-like-page(); }
            byline-clicked => { root.byline-clicked(); }
        }
        if row.kind == 5: Text {
            x: 8px;
            y: 8px;
            width: parent.width - 26px;
            height: 80px;
            text: row.title;
            color: Theme.dim;
            font-size: 13px;
            wrap: word-wrap;
            overflow: elide;
        }
        if row.kind == 6: TrackColumns {
            width: parent.width - 10px;
            show-album: !root.compact && root.main-width >= 760px;
        }
    }
}
```

The `show-album` condition is repeated, so fold it into one AppWindow property
`property <bool> show-album: main-width >= 760px;` (compact already implies < 700px, so the
second check alone is enough).

### 3.6 Skeletons

`SkeletonRow` gets a numbered variant (no art block, a short number bone instead):

```slint
component SkeletonRow inherits Rectangle {
    in property <int> seed;
    in property <bool> round;
    in property <bool> numbered;
    property <length> room: self.width - 64px - 14px;
    height: numbered ? 52px : 60px;
    if !root.numbered: Bone { x: 8px; y: 8px; width: 44px; height: 44px; corner: root.round ? 22px : 6px; }
    if root.numbered: Bone { x: 20px; y: 20px; width: 12px; height: 12px; corner: 3px; }
    Bone {
        x: root.numbered ? 48px : 64px;
        y: root.numbered ? 14px : 17px;
        width: root.room * (0.62 - mod(root.seed * 7, 5) * 0.055);
        height: 11px;
    }
    Bone {
        x: root.numbered ? 48px : 64px;
        y: root.numbered ? 32px : 36px;
        width: root.room * (0.34 - mod(root.seed * 3, 4) * 0.035);
        height: 9px;
    }
}
```

(Keep the numbered row's two bars, the second one shorter, matching a track with an artist
line. It is fine that real numbered rows are often one-line, the skeleton only needs to read
as "list".)

Detail skeleton. It copies the hero geometry from `DetailHeader` exactly (same pad, art size,
hero heights) so nothing jumps when the real hero replaces it:

```slint
// Stands in for a detail page while it loads: the hero card, then a list.
// banner is the artist layout, numbered the album/playlist track rows.
component SkeletonDetail inherits Rectangle {
    in property <bool> compact;
    in property <bool> banner;
    in property <bool> numbered: true;
    property <length> pad: compact ? 16px : 24px;
    property <length> art-size: compact ? 140px : 216px;
    property <length> hero: banner ? (compact ? 220px : 300px) : (compact ? 232px : 288px);
    property <length> text-x: banner ? pad : pad + art-size + (compact ? 16px : 24px);
    clip: true;
    Rectangle {
        width: parent.width - 10px;
        height: root.hero;
        border-radius: Theme.radius;
        background: #ffffff0a;
        if !root.banner: Bone {
            x: root.pad;
            y: root.pad;
            width: root.art-size;
            height: root.art-size;
            corner: 10px;
        }
        // Bars hang from the bottom of the art, like the real text does.
        Bone { x: root.text-x; y: root.hero - root.pad - 36px - 10px - 20px - 14px - 54px - 14px - 12px;
               width: 64px; height: 11px; }
        Bone { x: root.text-x; y: root.hero - root.pad - 36px - 10px - 20px - 14px - 54px;
               width: min(parent.width - root.text-x - root.pad, 340px); height: root.compact ? 30px : 44px; corner: 8px; }
        Bone { x: root.text-x; y: root.hero - root.pad - 36px - 10px - 20px - 6px;
               width: 120px; height: 12px; }
        Bone { x: root.text-x; y: root.hero - root.pad - 36px - 10px;
               width: 78px; height: 36px; corner: 18px; }
        Bone { x: root.text-x + 88px; y: root.hero - root.pad - 36px - 10px;
               width: root.compact ? 36px : 96px; height: 36px; corner: 18px; }
    }
    Bone { x: 8px; y: root.hero + 12px + 18px; width: 140px; height: 20px; corner: 6px; }
    for i in 8: SkeletonRow {
        x: 0;
        y: root.hero + 12px + 56px + i * (root.numbered ? 52px : 60px);
        width: root.width - 10px;
        numbered: root.numbered;
        seed: i + 2;
        visible: self.y < root.height;
    }
}
```

The bar y-offsets are chosen so their bottoms line up with the buttons row; adjust by eye on
first run (they are cosmetic only). The "Bone 11px / 44px / 12px" sizes match label / title /
meta text heights.

Use it where `SkeletonPage` is used now (same parent, same geometry; the existing
`if root.loading && root.rows.length == 0 && root.nav != 3: SkeletonPage {...}`):

```slint
if root.loading && root.rows.length == 0 && root.nav != 3 && root.header.kind != 0: SkeletonDetail {
    /* same x, y, width, height as the SkeletonPage next to it, plus padding-left 16px */
    compact: root.compact;
    banner: root.header.kind == 1;
    numbered: root.header.kind == 2 || root.header.kind == 4 || root.header.kind == 1;
}
if root.loading && root.rows.length == 0 && root.nav != 3 && root.header.kind == 0: SkeletonPage { ... }
```

`header.kind` is the *guessed* kind set by `open()` from the route before the request
(section 2.4), so the right skeleton shows immediately. Note `loading` stays true during the
append of continuation pages; the guard `rows.length == 0` keeps the skeleton to the first load.

---------------------------------------------------------------------------------------------

## 4. Behaviour notes

**Scrolling.** The hero is a normal `ListView` row (kind 4, first row) so it scrolls away with
the list. This is the only reliable way in Slint: a `ListView` cannot host a non-row header, and
a `Flickable` + hand-rolled virtualisation would lose `ListView`'s item recycling. The row's
height is a pure function of `kind` and `compact`, so there is no layout feedback into the
window (same constraint the existing code respects). The hero content is absolutely
positioned inside a fixed-size card.

**Optional sticky mini bar** (not in v1): `ListView` exposes `viewport-y`. When
`-list.viewport-y > hero-height - 40px`, fade in (opacity animated 150ms) a 48px bar overlaid at
the top of the island with title + Play pill. Needs `list := ListView` and the bar placed
after it in z-order. Cheap to add later, no data changes.

**Tint.** `Target::Hero` loads the art, `tint_of(&image, 120.0)` sets `page-tint`, and the
window animates `shown-page-tint` over 700ms exactly like the player. Until the art arrives the
default `#2b2440` is used; on a new page reset `page-tint` to the default in `open()` so the
previous page's colour does not flash.

**Narrow widths.** Island min is about 580px (820px window), minus the panel when open.
`compact` (< 700px) shrinks art to 140, title to 30/24, drops the Shuffle label to an icon,
hides the album column, and uses the 232px hero. The album column also needs >= 760px even on
playlists. Title and byline are `elide`d so nothing wraps; buttons are a fixed-width row. At 580px
the compact text column is about 580-32(pads)-140-16-16 = 376px: Play (~80) + shuffle icon (36)
+ Follow (~84) + like (36) + 3x10px spacing = 266px, fits.

**Hover/press.** Rows: existing `SongRow` hover (`Theme.hover`, 120ms) plus number-to-play-glyph.
Buttons: existing `PillButton`/`IconButton` hover states. Byline: turns `Theme.accent` and
shows a pointer cursor only when it is browsable. "Show all": pill background `Theme.hover`
and text brightens, 120ms. The hero itself is not clickable. Cards in carousels keep their
existing hover play badge.

**Actions.**
- Play -> `play-page(false)`. Existing `play_page` queues `page_songs()` from the first playable
  row; on an artist with `Header.play` the Rust handler can instead call
  `open_item(play)` for the artist radio.
- Shuffle -> `play-page(true)`.
- Follow / Like: hidden (`can-follow`/`can-like` false) until the client has the endpoints
  (subscribe: `subscription/subscribe` with `channelIds`, like: `like/like` with
  `target.playlistId` and `status: LIKE`, **guess, both need signed-in cookies**). The Slint
  callbacks and state flags exist from day one so that is a Rust-only change later.
- Keyboard: nothing new; existing app shortcuts apply.

**Loading and continuation.** First page: hero row + content. `kind 3` load-more rows still
trigger `load-more` via `Appear`; only page 1 emits the hero and column header. If the
response has a title but no recognised header renderer (Home sub-pages like moods), Rust sends
`header: None`, `kind 0`, and the old strip renders as today.

**Errors.** If the request fails (`show` -> `Err`), the toast fires as today; `loading` goes
false, the skeleton disappears, and `header.kind` is reset to 0 in the error branch so an empty
page does not show a hero-less skeleton state. Add `self.ui.set_header(PageHeader::default())` there.

**Accessibility/contrast.** The gradient is capped (peak 120 at 75% alpha) so white text on it
is at least ~4.5:1 for typical covers; the banner has a black bottom gradient for the same
reason.

---------------------------------------------------------------------------------------------

## 5. Implementation plan (ordered)

1. **model.rs**: add `PageKind`, `Header`, `Page.header`, `Shelf.more`, `Item.album`. Compile.
2. **parse.rs**
   a. `Walker::header(&mut self, key: &str, h: &Value)`; build `Header` per key (section 2.3),
      keep the existing title/subtitle/thumb assignment untouched.
   b. Add `thumb_best` and `split_dot_runs(&Value) -> Vec<(String, Option<String>)>` (run text +
      its `browseEndpoint.browseId`) helpers.
   c. `Walker::shelf`: read `more` from `bottomEndpoint` / `moreContentButton`.
   d. `list_item`: fill `Item.album` from the album-typed column.
   e. In `page()`: if `header.kind == Artist`, rename the "Songs" shelf to "Popular" and cap to
      5 when `more` is `Some`.
   f. Unit tests for an album header, an artist header, and the album column.
3. **images.rs**: `fetch_wide`, and the key format including w/h.
4. **app.slint** (do after main.rs is stable, to avoid colliding with the other editor):
   add structs/fields, properties, callbacks; replace `SongRow`; add `SectionHeader`,
   `TrackColumns`, `DetailHeader`, `SkeletonDetail`, numbered `SkeletonRow`; swap the `ListView`
   body and the old-strip condition; wire the skeleton. Build with
   `cargo check`; fix the `character-count` / clip passthrough if the compiler objects.
5. **main.rs**
   a. `tint_of` refactor of `set_tint`.
   b. `Target::Hero`, `PageState.{mores,header}`, `place_thumb` arm.
   c. `open()`: reset `header`, `page_tint`, set the guessed kind (`guess_kind(&Route)`).
   d. `show()`: convert `Page.header` to `PageHeader`, start the hero art load, error branch reset.
   e. `append()`: hero / column-header / text rows, `style` and `number`, `more`/`more_id`,
      skip art fetch for style 1, set `can_play/can_shuffle` after computing `playable`.
   f. `item_data()`: fill `album`.
   g. Register callbacks `show_all`, `byline_clicked`; stub `toggle_follow`/`toggle_like_page`.
6. **Manual check list**: album (square, numbered), compilation album (artist subtitle kept),
   single, artist with banner and without, playlist with 100+ tracks (load-more keeps working,
   Place indices still correct), podcast, profile, narrow window with the lyrics panel open
   (compact), Home/Search (unchanged strip), back navigation (hero restores because `open`
   re-requests; add the `header`/`page_tint` reset first), skeleton to hero swap (no jump).
7. **Later**: sticky mini bar, follow/like endpoints (`client.rs`-style new calls next to
   `browse`), clickable album names in the playlist column, expand-in-place for "Popular".
