//! Plain data types the rest of the app works with.

use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Song,
    Album,
    Artist,
    Playlist,
    #[default]
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub thumb: String,
    pub duration: String,
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
    pub browse_id: Option<String>,
    pub params: Option<String>,
    /// The artists named in the subtitle, with the browse id of their
    /// page when the subtitle links to one.
    pub artists: Vec<(String, Option<String>)>,
    /// The album named in the subtitle: its name and browse id.
    pub album: Option<(String, String)>,
    /// A song's place in a playlist, which removing it from there needs.
    pub set_video_id: Option<String>,
    /// An album or playlist that can be saved to the library: the playlist
    /// behind it, and whether it is saved already.
    pub library: Option<(String, bool)>,
}

impl Item {
    pub fn duration_secs(&self) -> f32 {
        let mut total = 0u32;
        for part in self.duration.split(':') {
            match part.trim().parse::<u32>() {
                Ok(n) => total = total * 60 + n,
                Err(_) => return 0.0,
            }
        }
        total as f32
    }

    /// The artist and album links as JSON, for the files that keep items.
    pub fn links_json(&self) -> (Value, Value) {
        let artists: Vec<Value> = self
            .artists
            .iter()
            .map(|(name, id)| json!([name, id]))
            .collect();
        let album = match &self.album {
            Some((name, id)) => json!([name, id]),
            None => Value::Null,
        };
        (Value::Array(artists), album)
    }

    /// Reads what `links_json` wrote. Files from before the links existed
    /// have neither key, which reads as no links.
    pub fn artists_from_json(v: Option<&Value>) -> Vec<(String, Option<String>)> {
        v.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let name = a.get(0)?.as_str()?.to_string();
                Some((name, a.get(1).and_then(Value::as_str).map(str::to_string)))
            })
            .collect()
    }

    pub fn album_from_json(v: Option<&Value>) -> Option<(String, String)> {
        let a = v?;
        Some((a.get(0)?.as_str()?.to_string(), a.get(1)?.as_str()?.to_string()))
    }

    /// The whole item as JSON, for the files that keep items.
    pub fn to_json(&self) -> Value {
        let (artist_links, album) = self.links_json();
        json!({
            "kind": self.kind.name(),
            "title": self.title,
            "subtitle": self.subtitle,
            "thumb": self.thumb,
            "duration": self.duration,
            "video": self.video_id,
            "playlist": self.playlist_id,
            "browse": self.browse_id,
            "params": self.params,
            "artist_links": artist_links,
            "album": album,
        })
    }

    /// Reads back what `to_json` wrote. Missing fields come back empty.
    pub fn from_json(e: &Value) -> Item {
        let s = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
        Item {
            kind: Kind::from_name(&s("kind").unwrap_or_default()),
            title: s("title").unwrap_or_default(),
            subtitle: s("subtitle").unwrap_or_default(),
            thumb: s("thumb").unwrap_or_default(),
            duration: s("duration").unwrap_or_default(),
            video_id: s("video"),
            playlist_id: s("playlist"),
            browse_id: s("browse"),
            params: s("params"),
            artists: Item::artists_from_json(e.get("artist_links")),
            album: Item::album_from_json(e.get("album")),
            ..Item::default()
        }
    }
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Song => "song",
            Kind::Album => "album",
            Kind::Artist => "artist",
            Kind::Playlist => "playlist",
            Kind::Other => "other",
        }
    }

    pub fn from_name(name: &str) -> Kind {
        match name {
            "song" => Kind::Song,
            "album" => Kind::Album,
            "artist" => Kind::Artist,
            "playlist" => Kind::Playlist,
            _ => Kind::Other,
        }
    }
}

/// What a detail page is about. Picks its layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageKind {
    Artist,
    Album,
    Single,
    Playlist,
    Podcast,
    Profile,
    #[default]
    Other,
}

/// The top of an artist, album or playlist page.
#[derive(Clone, Debug, Default)]
pub struct Header {
    pub kind: PageKind,
    /// "Album", "EP", "Playlist" and so on.
    pub label: String,
    pub title: String,
    /// The artist or owner.
    pub byline: String,
    pub byline_browse_id: Option<String>,
    /// "2013 • 13 songs • 1 hour, 14 minutes", or the audience of an artist.
    pub meta: String,
    pub about: String,
    /// Square art, or the wide banner of an artist.
    pub thumb: String,
    pub banner: bool,
    /// Some when the page has a subscribe button, with the channel it follows.
    pub subscribed: Option<bool>,
    pub channel_id: Option<String>,
    /// Some when the page can be saved to the library.
    pub liked: Option<bool>,
    /// The playlist behind an album or playlist, for saving it.
    pub playlist_id: Option<String>,
    /// A playlist of the user's own, which they can rename, delete and
    /// take songs out of.
    pub editable: bool,
    /// The artist's shuffle and radio, as YouTube Music starts them.
    pub shuffle: Option<Item>,
    pub radio: Option<Item>,
}

#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub title: String,
    /// Horizontal row of cards when true, vertical list when false.
    pub carousel: bool,
    pub items: Vec<Item>,
    /// The page behind "Show all": browse id and params.
    pub more: Option<(String, Option<String>)>,
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub title: String,
    pub subtitle: String,
    pub thumb: String,
    pub shelves: Vec<Shelf>,
    pub continuation: Option<String>,
    /// Only artist, album and playlist pages have one.
    pub header: Option<Header>,
}
