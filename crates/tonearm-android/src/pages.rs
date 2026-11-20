//! YouTube Music's pages as the Android app draws them: Home, Library,
//! search, and the page of an album, artist or playlist. Each is shelves of
//! items, as on the desktop. The core's own types, with what Kotlin needs.

use tonearm_core::innertube::{self, model};

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Debug)]
pub enum ItemKind {
    Song,
    Album,
    Artist,
    Playlist,
    Other,
}

/// Anything on a page: a song, an album, an artist, a playlist, a mix.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub title: String,
    pub subtitle: String,
    /// The art's link, sized for a card.
    pub thumb: String,
    /// "3:12" for a song. Empty when unknown.
    pub duration: String,
    pub video_id: Option<String>,
    pub browse_id: Option<String>,
    pub playlist_id: Option<String>,
    pub params: Option<String>,
    /// Its artists, joined with commas, for a song.
    pub artist: String,
    pub album: String,
    pub explicit: bool,
    /// Each credited artist, with their page where YouTube Music links it,
    /// for a song's "Go to artist".
    pub artists: Vec<Link>,
    /// The album's page, for "Go to album".
    pub album_id: Option<String>,
    /// An album or playlist that can be saved to the library: the playlist
    /// behind it, and whether it is saved already.
    pub library_playlist: Option<String>,
    pub saved: bool,
    /// A mood or genre's colour (0xAARRGGBB); None for everything else.
    pub color: Option<u32>,
    /// On a chart, how the song's place moved since the last one.
    pub trend: Trend,
}

/// A chart song's move, marked beside its rank.
#[derive(uniffi::Enum, Clone, Copy, PartialEq, Debug)]
pub enum Trend {
    /// Not on a chart.
    None,
    Up,
    Down,
    Same,
}

/// A name and, where there is one, the page it opens.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct Link {
    pub name: String,
    pub id: Option<String>,
}

pub fn links(artists: &[(String, Option<String>)]) -> Vec<Link> {
    artists
        .iter()
        .map(|(name, id)| Link {
            name: name.clone(),
            id: id.clone(),
        })
        .collect()
}

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Debug)]
pub enum ShelfLayout {
    /// A list of rows, down the page.
    List,
    /// Square cards, side by side.
    Cards,
    /// Rows of songs, four to a column, side by side.
    Grid,
    /// Wide cards, side by side, for videos.
    Wide,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Shelf {
    pub title: String,
    pub layout: ShelfLayout,
    pub items: Vec<Item>,
    /// The page behind "Show all", if any.
    pub more_id: Option<String>,
    pub more_params: Option<String>,
    /// Its rows carry their rank before the art: Explore's top songs, or a
    /// chart's songs.
    pub ranked: bool,
}

/// The top of an album, artist or playlist page.
#[derive(uniffi::Record, Clone, Debug)]
pub struct Header {
    /// "Album", "EP", "Playlist" and so on.
    pub label: String,
    pub title: String,
    /// The artist or owner.
    pub byline: String,
    pub byline_id: Option<String>,
    /// "2013 • 13 songs • 1 hour".
    pub meta: String,
    pub about: String,
    pub thumb: String,
    /// The thumb is an artist's wide banner, not square art.
    pub banner: bool,
    /// What the play and shuffle buttons start, for an artist.
    pub shuffle: Option<Item>,
    pub radio: Option<Item>,
    pub playlist_id: Option<String>,
}

/// One of Home's moods, such as "Relax".
#[derive(uniffi::Record, Clone, Debug)]
pub struct Chip {
    pub title: String,
    pub browse_id: String,
    pub params: Option<String>,
    pub selected: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Page {
    pub title: String,
    pub header: Option<Header>,
    pub shelves: Vec<Shelf>,
    pub chips: Vec<Chip>,
    /// More shelves come from `more` with this.
    pub continuation: Option<String>,
}

/// A link with its scheme, at about `px` pixels.
fn art(url: &str, px: u32) -> String {
    if url.is_empty() {
        return String::new();
    }
    let url = innertube::sized(url, px);
    if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url
    }
}

pub fn item(i: &model::Item) -> Item {
    let artist = if i.artists.is_empty() {
        String::new()
    } else {
        i.artists.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ")
    };
    Item {
        kind: match i.kind {
            model::Kind::Song => ItemKind::Song,
            model::Kind::Album => ItemKind::Album,
            model::Kind::Artist => ItemKind::Artist,
            model::Kind::Playlist => ItemKind::Playlist,
            model::Kind::Other => ItemKind::Other,
        },
        title: i.title.clone(),
        subtitle: i.subtitle.clone(),
        thumb: art(&i.thumb, 544),
        duration: i.duration.clone(),
        video_id: i.video_id.clone(),
        browse_id: i.browse_id.clone(),
        playlist_id: i.playlist_id.clone(),
        params: i.params.clone(),
        artist,
        album: i.album.as_ref().map(|(name, _)| name.clone()).unwrap_or_default(),
        explicit: i.explicit,
        artists: links(&i.artists),
        album_id: i.album.as_ref().map(|(_, id)| id.clone()),
        library_playlist: i.library.as_ref().map(|(playlist, _)| playlist.clone()),
        saved: i.library.as_ref().is_some_and(|(_, saved)| *saved),
        color: i.color,
        trend: match i.trend {
            model::Trend::None => Trend::None,
            model::Trend::Up => Trend::Up,
            model::Trend::Down => Trend::Down,
            model::Trend::Same => Trend::Same,
        },
    }
}

/// Back to the core's item, to play or open it. What the core reads is all
/// here; the rest it doesn't need.
pub fn core_item(i: &Item) -> model::Item {
    model::Item {
        kind: match i.kind {
            ItemKind::Song => model::Kind::Song,
            ItemKind::Album => model::Kind::Album,
            ItemKind::Artist => model::Kind::Artist,
            ItemKind::Playlist => model::Kind::Playlist,
            ItemKind::Other => model::Kind::Other,
        },
        title: i.title.clone(),
        subtitle: i.subtitle.clone(),
        thumb: i.thumb.clone(),
        duration: i.duration.clone(),
        video_id: i.video_id.clone(),
        browse_id: i.browse_id.clone(),
        playlist_id: i.playlist_id.clone(),
        params: i.params.clone(),
        explicit: i.explicit,
        artists: i.artists.iter().map(|a| (a.name.clone(), a.id.clone())).collect(),
        album: i.album_id.clone().map(|id| (i.album.clone(), id)),
        library: i.library_playlist.clone().map(|playlist| (playlist, i.saved)),
        ..model::Item::default()
    }
}

fn shelf(s: &model::Shelf) -> Shelf {
    Shelf {
        title: s.title.clone(),
        layout: if !s.carousel {
            ShelfLayout::List
        } else {
            match s.layout {
                model::Layout::Grid => ShelfLayout::Grid,
                model::Layout::Wide => ShelfLayout::Wide,
                model::Layout::Cards | model::Layout::Wrapped => ShelfLayout::Cards,
            }
        },
        items: s.items.iter().map(item).collect(),
        more_id: s.more.as_ref().map(|(id, _)| id.clone()),
        more_params: s.more.as_ref().and_then(|(_, params)| params.clone()),
        ranked: !s.carousel
            && (tonearm_core::explore::is_chart(s)
                || s.items.iter().any(|i| i.trend != model::Trend::None)),
    }
}

pub fn page(p: &model::Page) -> Page {
    Page {
        title: p.title.clone(),
        header: p.header.as_ref().map(|h| Header {
            label: h.label.clone(),
            title: h.title.clone(),
            byline: h.byline.clone(),
            byline_id: h.byline_browse_id.clone(),
            meta: h.meta.clone(),
            about: h.about.clone(),
            thumb: art(&h.thumb, 1080),
            banner: h.banner,
            shuffle: h.shuffle.as_ref().map(item),
            radio: h.radio.as_ref().map(item),
            playlist_id: h.playlist_id.clone(),
        }),
        shelves: p.shelves.iter().filter(|s| !s.items.is_empty()).map(shelf).collect(),
        chips: p
            .chips
            .iter()
            .map(|c| Chip {
                title: c.title.clone(),
                browse_id: c.browse_id.clone(),
                params: c.params.clone(),
                selected: c.selected,
            })
            .collect(),
        continuation: p.continuation.clone(),
    }
}
