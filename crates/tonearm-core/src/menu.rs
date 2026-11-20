//! What an item's menu offers: a right click on the desktop, a long press
//! on the phone. Both apps list these lines in this order, and add their
//! own after them (the desktop's Pin, Sync and copy lines).
//!
//! Also the fetching a line leads to that isn't one call: an album's or
//! playlist's songs, and an artist's shuffle or radio.

use crate::innertube::model::{Item, Kind, Page};
use crate::innertube::Client;
use crate::{offline, spotify};

/// What the app knows that the lines depend on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    pub signed_in: bool,
    /// The item is the song playing.
    pub playing: bool,
    /// The song playing is liked, as its heart shows.
    pub liked: bool,
    /// The menu is on the library or one of its lists, where the artists
    /// listed are followed. Their menus offer Unfollow.
    pub in_library: bool,
    /// The app keeps downloads, so offers to download.
    pub downloads: bool,
}

/// A line's icon, which each app draws from its own set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Shuffle,
    PlayNext,
    AddQueue,
    Artist,
    Album,
    Radio,
    Download,
    Trash,
    Heart,
    Open,
    Library,
}

/// What a line does, to the item its menu is for unless it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    PlayNext,
    AddToQueue,
    /// Opens a page: an artist's, an album's, the item's own.
    Browse(String),
    /// The song, then its radio.
    SongRadio,
    /// An artist's radio, by their page's id.
    ArtistRadio(String),
    /// An artist's shuffle, by their page's id.
    ArtistShuffle(String),
    /// Downloads the song, or deletes its download.
    ToggleDownload,
    /// Likes a song that isn't the one playing.
    Like,
    /// Likes or unlikes the song playing, as its heart does.
    LikePlaying,
    /// Lists the user's playlists, to put the song in one.
    AddToPlaylist,
    /// Fetches an album's or playlist's songs and plays them.
    Play,
    Shuffle,
    DownloadAll,
    /// Saves an album or playlist to the library, or takes it out.
    Library { playlist: String, save: bool },
    /// Follows an artist's channel, or stops.
    Follow { channel: String, follow: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub label: String,
    pub icon: Icon,
    pub action: Action,
}

/// An album or playlist that can be fetched for its songs from YouTube
/// Music. A Spotify playlist cannot: its songs are matched as its page
/// opens, so its menu offers Open instead.
pub fn is_collection(item: &Item) -> bool {
    let spotify = item.browse_id.as_deref().is_some_and(|id| id.starts_with(spotify::PREFIX));
    item.video_id.is_none()
        && !spotify
        && (matches!(item.kind, Kind::Album | Kind::Playlist)
            || item.browse_id.as_deref().is_some_and(|id| {
                id.starts_with("MPREb_") || id.starts_with("VL") || id.starts_with("OLAK")
            }))
}

/// A page whose artists are all followed: the library or one of its
/// lists, by its browse id.
pub fn is_library_page(id: &str) -> bool {
    id.starts_with("FEmusic_library") || id.starts_with("FEmusic_liked")
}

/// The lines of `item`'s menu: a song's, an artist's, an album's or
/// playlist's, or Open for anything else with a page.
pub fn lines(item: &Item, context: &Context) -> Vec<Line> {
    let mut out = Vec::new();
    let mut line = |label: &str, icon: Icon, action: Action| {
        out.push(Line {
            label: label.to_string(),
            icon,
            action,
        })
    };
    // Each linked artist, with their page.
    let linked: Vec<(&str, &str)> = item
        .artists
        .iter()
        .filter_map(|(name, id)| Some((name.as_str(), id.as_deref()?)))
        .collect();
    let several = linked.len() > 1;
    if let Some(video_id) = &item.video_id {
        if !context.playing {
            line("Play next", Icon::PlayNext, Action::PlayNext);
            line("Add to queue", Icon::AddQueue, Action::AddToQueue);
        }
        for (name, id) in &linked {
            let label = if several { format!("Go to {name}") } else { "Go to artist".into() };
            line(&label, Icon::Artist, Action::Browse(id.to_string()));
        }
        if let Some((_, id)) = &item.album {
            line("Go to album", Icon::Album, Action::Browse(id.clone()));
        }
        line("Song radio", Icon::Radio, Action::SongRadio);
        // Each linked artist's own radio, as Spotify offers.
        for (name, id) in &linked {
            let label = if several { format!("{name} radio") } else { "Artist radio".into() };
            line(&label, Icon::Radio, Action::ArtistRadio(id.to_string()));
        }
        if context.downloads {
            if offline::is_downloaded(video_id) {
                line("Remove download", Icon::Trash, Action::ToggleDownload);
            } else if !offline::is_downloading(video_id) {
                line("Download", Icon::Download, Action::ToggleDownload);
            }
        }
        if context.signed_in {
            if context.playing {
                let label = if context.liked { "Unlike" } else { "Like" };
                line(label, Icon::Heart, Action::LikePlaying);
            } else {
                line("Like", Icon::Heart, Action::Like);
            }
            line("Add to playlist", Icon::AddQueue, Action::AddToPlaylist);
        }
    } else if item.kind == Kind::Artist {
        if let Some(id) = &item.browse_id {
            line("Open", Icon::Open, Action::Browse(id.clone()));
            line("Shuffle", Icon::Shuffle, Action::ArtistShuffle(id.clone()));
            line("Start radio", Icon::Radio, Action::ArtistRadio(id.clone()));
            // An artist's page id is their channel. The menu does not say
            // whether they are followed, but the library only lists
            // artists that are.
            if context.signed_in && id.starts_with("UC") {
                let follow = !context.in_library;
                let label = if follow { "Follow" } else { "Unfollow" };
                line(
                    label,
                    Icon::Artist,
                    Action::Follow {
                        channel: id.clone(),
                        follow,
                    },
                );
            }
        }
    } else if is_collection(item) {
        line("Play", Icon::Play, Action::Play);
        line("Shuffle", Icon::Shuffle, Action::Shuffle);
        if context.downloads {
            line("Download all", Icon::Download, Action::DownloadAll);
        }
        for (name, id) in &linked {
            let label = if several { format!("Go to {name}") } else { "Go to artist".into() };
            line(&label, Icon::Artist, Action::Browse(id.to_string()));
        }
        if let Some(id) = &item.browse_id {
            line("Open", Icon::Open, Action::Browse(id.clone()));
        }
        if let Some((playlist, saved)) = item.library.clone().filter(|_| context.signed_in) {
            let (label, icon) = if saved {
                ("Remove from library", Icon::Trash)
            } else {
                ("Save to library", Icon::Library)
            };
            line(label, icon, Action::Library { playlist, save: !saved });
        }
    } else if let Some(id) = &item.browse_id {
        line("Open", Icon::Open, Action::Browse(id.clone()));
    }
    out
}

/// What a page lists down it, not in its carousels: an album's tracks, a
/// playlist's songs, an artist's top songs.
pub fn songs_of(page: &Page) -> Vec<Item> {
    page.shelves
        .iter()
        .filter(|s| !s.carousel)
        .flat_map(|s| s.items.iter())
        .filter(|i| i.video_id.is_some())
        .cloned()
        .collect()
}

/// The songs of an album or playlist, every page of them, for Play and
/// Shuffle. Blocks.
pub fn collection_songs(client: &Client, item: &Item) -> Result<Vec<Item>, String> {
    if let Some(id) = &item.browse_id {
        let page = client.browse(id, item.params.as_deref())?;
        let mut songs = songs_of(&page);
        // A long playlist comes in pages. Fetch them all, within reason.
        let mut token = page.continuation;
        for _ in 0..40 {
            let Some(t) = token.take() else { break };
            let more = client.more(&t)?;
            songs.extend(songs_of(&more));
            token = more.continuation;
        }
        Ok(songs)
    } else if let Some(list) = &item.playlist_id {
        Ok(client.queue(None, Some(list))?.0)
    } else {
        Err("Nothing to play".into())
    }
}

/// An artist's shuffle or radio, as their page offers it, or the songs on
/// their page when it has neither. Blocks.
pub fn artist_songs(client: &Client, id: &str, shuffle: bool) -> Result<Vec<Item>, String> {
    let page = client.browse(id, None)?;
    let header = page.header.clone().unwrap_or_default();
    let endpoint = if shuffle { header.shuffle } else { header.radio };
    match endpoint {
        Some(e) => client
            .queue_with(e.video_id.as_deref(), e.playlist_id.as_deref(), e.params.as_deref())
            .map(|(items, _)| items),
        None => Ok(songs_of(&page)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(item: &Item, context: &Context) -> Vec<String> {
        lines(item, context).into_iter().map(|l| l.label).collect()
    }

    #[test]
    fn a_song_offers_its_artists_and_album() {
        let song = Item {
            kind: Kind::Song,
            video_id: Some("v".into()),
            artists: vec![("A".into(), Some("UCa".into())), ("B".into(), None)],
            album: Some(("Album".into(), "MPREb_1".into())),
            ..Item::default()
        };
        let signed_in = Context { signed_in: true, ..Context::default() };
        assert_eq!(
            labels(&song, &signed_in),
            ["Play next", "Add to queue", "Go to artist", "Go to album", "Song radio", "Artist radio", "Like", "Add to playlist"]
        );
        // The song playing is liked or not, as its heart says.
        let playing = Context { playing: true, liked: true, ..signed_in };
        assert_eq!(labels(&song, &playing), ["Go to artist", "Go to album", "Song radio", "Artist radio", "Unlike", "Add to playlist"]);
    }

    #[test]
    fn an_artist_offers_follow_outside_the_library() {
        let artist = Item {
            kind: Kind::Artist,
            browse_id: Some("UCx".into()),
            ..Item::default()
        };
        let context = Context { signed_in: true, ..Context::default() };
        assert_eq!(labels(&artist, &context), ["Open", "Shuffle", "Start radio", "Follow"]);
        let library = Context { in_library: true, ..context };
        assert_eq!(labels(&artist, &library).last().unwrap(), "Unfollow");
        assert_eq!(labels(&artist, &Context::default()), ["Open", "Shuffle", "Start radio"]);
    }

    #[test]
    fn an_album_offers_play_and_saving() {
        let album = Item {
            kind: Kind::Album,
            browse_id: Some("MPREb_1".into()),
            artists: vec![("A".into(), Some("UCa".into())), ("B".into(), Some("UCb".into()))],
            library: Some(("OLAK1".into(), true)),
            ..Item::default()
        };
        let context = Context { signed_in: true, ..Context::default() };
        assert_eq!(labels(&album, &context), ["Play", "Shuffle", "Go to A", "Go to B", "Open", "Remove from library"]);
        let downloads = Context { downloads: true, ..context };
        assert_eq!(labels(&album, &downloads)[2], "Download all");
    }
}
