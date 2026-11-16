//! Detail pages: the hero of an artist, album or playlist page, and the
//! links in subtitles that lead to those pages.

use crate::innertube::model::{Header, Item, Page, PageKind};
use crate::{on_ui, App, PageHeader, Place, Route, Run};
use slint::{Model, ModelRc, VecModel};
use std::rc::Rc;

/// The label of the Downloads hero. It also marks the hero that gets the
/// download icon tile instead of art.
const DOWNLOADS_LABEL: &str = "Saved on this computer";

/// `PageHeader.kind` for a page kind.
fn kind_number(kind: PageKind) -> i32 {
    match kind {
        PageKind::Artist => 1,
        PageKind::Album => 2,
        PageKind::Playlist => 3,
        PageKind::Single => 4,
        PageKind::Podcast => 5,
        PageKind::Profile => 6,
        PageKind::Other => 7,
    }
}

/// What kind of page a link opens, from its browse id alone. Picks the
/// skeleton while the page loads. 0 is a plain page.
pub fn guess_kind(route: &Route) -> i32 {
    match route {
        Route::Downloads => 3,
        Route::Browse { id, .. } if id.starts_with("UC") => 1,
        Route::Browse { id, .. } if id.starts_with("MPREb_") => 2,
        Route::Browse { id, .. } if id.starts_with("VL") || id.starts_with("PL") => 3,
        Route::Browse { id, .. } if id.starts_with("MPSP") => 5,
        _ => 0,
    }
}

/// The Downloads page looks like a playlist of what is saved.
pub fn downloads_header(page: &Page) -> Header {
    Header {
        kind: PageKind::Playlist,
        label: DOWNLOADS_LABEL.into(),
        title: page.title.clone(),
        meta: page.subtitle.clone(),
        ..Header::default()
    }
}

/// The hero as the window draws it. The art comes later, once it loads.
pub fn header_data(header: &Header, signed_in: bool) -> PageHeader {
    let artist = header.kind == PageKind::Artist;
    PageHeader {
        kind: kind_number(header.kind),
        label: header.label.as_str().into(),
        title: header.title.as_str().into(),
        byline: header.byline.as_str().into(),
        byline_browsable: header.byline_browse_id.is_some(),
        meta: header.meta.as_str().into(),
        round: artist,
        banner: header.banner,
        icon_tile: header.label == DOWNLOADS_LABEL && header.thumb.is_empty(),
        // Following and saving change the account, so they need a sign in.
        can_follow: signed_in && header.subscribed.is_some() && header.channel_id.is_some(),
        followed: header.subscribed == Some(true),
        can_like: signed_in && header.liked.is_some() && header.playlist_id.is_some(),
        liked: header.liked == Some(true),
        ..PageHeader::default()
    }
}

/// Cuts a subtitle into runs, so the artist and album names in it can be
/// clicked. Gives the runs, and the runs without the album, for rows that
/// show the album in a column of its own. Both are empty when nothing in
/// the text links anywhere, and the plain text shows instead.
pub fn link_runs(text: &str, item: &Item) -> (Vec<Run>, Vec<Run>) {
    let mut runs: Vec<(String, String, bool)> = Vec::new();
    let mut rest = text;
    let mut linked = false;
    let plain = |runs: &mut Vec<(String, String, bool)>, s: &str| {
        if !s.is_empty() {
            runs.push((s.to_string(), String::new(), false));
        }
    };
    for (name, id) in &item.artists {
        let Some(id) = id else { continue };
        if let Some(at) = rest.find(name.as_str()) {
            plain(&mut runs, &rest[..at]);
            runs.push((name.clone(), id.clone(), false));
            rest = &rest[at + name.len()..];
            linked = true;
        }
    }
    if let Some((name, id)) = &item.album {
        if let Some(at) = rest.find(name.as_str()) {
            let before = &rest[..at];
            // The dot before the album goes with it, so hiding the album
            // hides both.
            match before.strip_suffix(" • ") {
                Some(head) => {
                    plain(&mut runs, head);
                    runs.push((" • ".into(), String::new(), true));
                }
                None => plain(&mut runs, before),
            }
            runs.push((name.clone(), id.clone(), true));
            rest = &rest[at + name.len()..];
            linked = true;
        }
    }
    plain(&mut runs, rest);
    if !linked {
        return (Vec::new(), Vec::new());
    }
    // Each run carries the text before it, so the window can measure
    // where it starts on the line.
    let build = |pieces: Vec<&(String, String, bool)>| -> Vec<Run> {
        let mut before = String::new();
        pieces
            .into_iter()
            .map(|(text, browse, _)| {
                let run = Run {
                    text: text.as_str().into(),
                    browse: browse.as_str().into(),
                    before: before.as_str().into(),
                };
                before.push_str(text);
                run
            })
            .collect()
    };
    let full = build(runs.iter().collect());
    let short = build(runs.iter().filter(|r| !r.2).collect());
    (full, short)
}

/// True when every artist of a track is the album's own artist.
pub fn only_by(item: &Item, byline: &str) -> bool {
    !byline.is_empty()
        && (item.artists.is_empty() || item.artists.iter().all(|(name, _)| name == byline))
}

impl App {
    pub fn show_all(self: &Rc<Self>, id: usize) {
        let more = self.page.borrow().mores.get(id).cloned();
        if let Some((id, params)) = more {
            self.open(Route::Browse { id, params }, true);
        }
    }

    pub fn byline_clicked(self: &Rc<Self>) {
        let id = self
            .page
            .borrow()
            .header
            .as_ref()
            .and_then(|h| h.byline_browse_id.clone());
        if let Some(id) = id {
            self.open_browse(&id);
        }
    }

    /// Opens the page an artist or album name links to.
    pub fn open_browse(self: &Rc<Self>, id: &str) {
        if id.is_empty() {
            return;
        }
        self.open(
            Route::Browse {
                id: id.to_string(),
                params: None,
            },
            true,
        );
    }

    /// Follows the artist on this page, or stops. The button flips at once
    /// and flips back if YouTube Music says no.
    pub fn toggle_follow(self: &Rc<Self>) {
        let (channel, follow, generation) = {
            let mut page = self.page.borrow_mut();
            let generation = page.generation;
            let Some(header) = page.header.as_mut() else { return };
            let Some(channel) = header.channel_id.clone() else { return };
            let follow = header.subscribed != Some(true);
            header.subscribed = Some(follow);
            (channel, follow, generation)
        };
        let mut data = self.ui.get_header();
        data.followed = follow;
        self.ui.set_header(data);
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.subscribe(&channel, follow);
            on_ui(move |app| {
                if let Err(message) = result {
                    app.toast(message);
                    if app.page.borrow().generation == generation {
                        if let Some(h) = app.page.borrow_mut().header.as_mut() {
                            h.subscribed = Some(!follow);
                        }
                        let mut data = app.ui.get_header();
                        data.followed = !follow;
                        app.ui.set_header(data);
                    }
                }
            });
        });
    }

    /// Saves the album or playlist on this page to the library, or takes it out.
    pub fn toggle_like_page(self: &Rc<Self>) {
        let (playlist, like, generation) = {
            let mut page = self.page.borrow_mut();
            let generation = page.generation;
            let Some(header) = page.header.as_mut() else { return };
            let Some(playlist) = header.playlist_id.clone() else { return };
            let like = header.liked != Some(true);
            header.liked = Some(like);
            (playlist, like, generation)
        };
        let mut data = self.ui.get_header();
        data.liked = like;
        self.ui.set_header(data);
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.rate_playlist(&playlist, like);
            on_ui(move |app| match result {
                Ok(()) => app.toast(if like {
                    "Saved to your library"
                } else {
                    "Removed from your library"
                }),
                Err(message) => {
                    app.toast(message);
                    if app.page.borrow().generation == generation {
                        if let Some(h) = app.page.borrow_mut().header.as_mut() {
                            h.liked = Some(!like);
                        }
                        let mut data = app.ui.get_header();
                        data.liked = !like;
                        app.ui.set_header(data);
                    }
                }
            });
        });
    }

    /// The links under the playing track's title: its artists and album.
    pub fn show_track_links(&self, item: &Item, artist: &str) {
        let album = self.ui.get_track_album();
        let line = if album.is_empty() {
            artist.to_string()
        } else {
            format!("{artist} • {album}")
        };
        let (runs, _) = link_runs(&line, item);
        // Shows the plain artist when nothing links.
        self.ui.set_track_runs(ModelRc::new(VecModel::from(runs)));
    }

    /// Plays what a watch endpoint names, such as an artist's shuffle or
    /// radio, with its params.
    pub fn play_endpoint(self: &Rc<Self>, endpoint: Item) {
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.queue_with(
                endpoint.video_id.as_deref(),
                endpoint.playlist_id.as_deref(),
                endpoint.params.as_deref(),
            );
            on_ui(move |app| match result {
                Ok((items, _)) if !items.is_empty() => app.set_queue(items, 0),
                Ok(_) => app.toast("Nothing to play"),
                Err(message) => app.toast(message),
            });
        });
    }

    /// Updates the downloaded marks on the page and in the queue. Runs
    /// whenever a download finishes or is removed.
    pub fn refresh_downloaded(&self) {
        let mark = |model: &VecModel<crate::ItemData>, index: usize, item: &Item| {
            let saved = item
                .video_id
                .as_deref()
                .is_some_and(crate::offline::is_downloaded);
            if let Some(mut data) = model.row_data(index) {
                if data.downloaded != saved {
                    data.downloaded = saved;
                    model.set_row_data(index, data);
                }
            }
        };
        {
            let page = self.page.borrow();
            for (item, place) in page.items.iter().zip(&page.places) {
                match place {
                    Place::Row(row) => {
                        let saved = item
                            .video_id
                            .as_deref()
                            .is_some_and(crate::offline::is_downloaded);
                        if let Some(mut data) = page.rows.row_data(*row) {
                            if data.item.downloaded != saved {
                                data.item.downloaded = saved;
                                page.rows.set_row_data(*row, data);
                            }
                        }
                    }
                    Place::Card(model, index) => mark(model, *index, item),
                }
            }
        }
        let player = self.player.borrow();
        for (index, item) in player.queue.iter().enumerate() {
            mark(&player.model, index, item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(subtitle: &str) -> Item {
        Item {
            subtitle: subtitle.into(),
            artists: vec![
                ("Daft Punk".into(), Some("UC1".into())),
                ("Pharrell Williams".into(), Some("UC2".into())),
            ],
            album: Some(("Random Access Memories".into(), "MPREb_1".into())),
            ..Item::default()
        }
    }

    #[test]
    fn names_in_a_subtitle_become_links() {
        let item = song("Daft Punk & Pharrell Williams • Random Access Memories • 6:10");
        let (full, short) = link_runs(&item.subtitle, &item);
        let texts: Vec<String> = full.iter().map(|r| r.text.to_string()).collect();
        assert_eq!(
            texts,
            ["Daft Punk", " & ", "Pharrell Williams", " • ", "Random Access Memories", " • 6:10"]
        );
        assert_eq!(full[0].browse, "UC1");
        assert_eq!(full[4].browse, "MPREb_1");
        assert_eq!(full[2].before, "Daft Punk & ");
        let short: Vec<String> = short.iter().map(|r| r.text.to_string()).collect();
        assert_eq!(short, ["Daft Punk", " & ", "Pharrell Williams", " • 6:10"]);
    }

    #[test]
    fn a_subtitle_with_no_links_stays_plain() {
        let item = Item {
            subtitle: "Song • 3:32".into(),
            ..Item::default()
        };
        assert!(link_runs(&item.subtitle, &item).0.is_empty());
    }
}
