//! "Recommended" under a playlist of the user's own: songs like the ones in
//! it, each with a button that adds it. Refresh shows ten more.

use crate::innertube::model::{Item, PageKind};
use crate::{on_ui, App, Place, RowData, Target};
use slint::Model;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

/// How many songs show at a time.
const SHOWN: usize = 10;
/// How many songs of the playlist the suggestions are drawn from. Each one
/// is a radio fetched, so a few, picked afresh each refresh.
const SEEDS: usize = 3;

/// The section on the open page.
#[derive(Default)]
pub struct Recommend {
    /// The section's first row: its heading. The section runs to the end.
    first_row: usize,
    /// Page item ids of the songs shown, to retire on refresh.
    ids: Vec<usize>,
    /// Every song shown so far, so a refresh brings new ones.
    shown: HashSet<String>,
    /// Counts up each fetch, so only the latest shows.
    token: u64,
}

/// Songs whose sound is being fetched to work out tempo and key.
fn fetching() -> std::sync::MutexGuard<'static, HashSet<String>> {
    static FETCHING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FETCHING.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the song's sound is on its way to the analyzer.
pub fn is_fetching(video_id: &str) -> bool {
    fetching().contains(video_id)
}

impl App {
    /// Starts the section once the whole of a playlist of the user's own is
    /// in. Does nothing if the page has it already.
    pub fn offer_recommended(self: &Rc<Self>) {
        {
            let page = self.page.borrow();
            let own = page
                .header
                .as_ref()
                .is_some_and(|h| h.kind == PageKind::Playlist && h.editable && h.playlist_id.is_some());
            if !own || page.continuation.is_some() || page.recommend.is_some() {
                return;
            }
        }
        if self.page_songs().is_empty() {
            return;
        }
        let first_row = self.page.borrow().rows.row_count();
        self.page.borrow_mut().recommend = Some(Recommend {
            first_row,
            ..Recommend::default()
        });
        self.refresh_recommended();
    }

    /// Fetches songs for the section, other than those in the playlist or
    /// shown before.
    pub fn refresh_recommended(self: &Rc<Self>) {
        let songs: Vec<Item> = self.page_songs().into_iter().map(|(_, s)| s).collect();
        let (generation, token, mut skip) = {
            let mut page = self.page.borrow_mut();
            let generation = page.generation;
            let Some(rec) = page.recommend.as_mut() else {
                return;
            };
            rec.token += 1;
            (generation, rec.token, rec.shown.clone())
        };
        skip.extend(songs.iter().filter_map(|s| s.video_id.clone()));
        self.show_recommended(Vec::new(), true);
        let seeds: Vec<String> = crate::shuffled(songs)
            .into_iter()
            .filter_map(|s| s.video_id)
            .take(SEEDS)
            .collect();
        let client = self.client.clone();
        self.api.run(move || {
            // Each seed's radio, taken in turns so no one song leads.
            let radios: Vec<Vec<Item>> = seeds
                .iter()
                .filter_map(|id| client.queue(Some(id), None).ok())
                .map(|(items, _)| items)
                .collect();
            let mut picks = Vec::new();
            let mut seen = skip;
            let longest = radios.iter().map(Vec::len).max().unwrap_or(0);
            'all: for i in 0..longest {
                for radio in &radios {
                    let Some(item) = radio.get(i) else { continue };
                    let Some(id) = item.video_id.clone() else { continue };
                    if seen.insert(id) {
                        picks.push(item.clone());
                        if picks.len() == SHOWN {
                            break 'all;
                        }
                    }
                }
            }
            on_ui(move |app| {
                let current = {
                    let page = app.page.borrow();
                    page.generation == generation && page.recommend.as_ref().is_some_and(|r| r.token == token)
                };
                if current {
                    app.show_recommended(picks, false);
                }
            });
        });
    }

    /// Puts the section's rows at the end of the page: its heading, `songs`,
    /// and the Refresh button, which waits while `loading`.
    fn show_recommended(self: &Rc<Self>, songs: Vec<Item>, loading: bool) {
        let (rows, generation, first_row, mut next_id) = {
            let mut page = self.page.borrow_mut();
            let next_id = page.items.len();
            let Some(rec) = page.recommend.as_mut() else {
                return;
            };
            let old = std::mem::take(&mut rec.ids);
            let first_row = rec.first_row;
            // The old songs' rows go. Their art may still arrive.
            for id in old {
                page.places[id] = Place::Gone;
            }
            (page.rows.clone(), page.generation, first_row, next_id)
        };
        while rows.row_count() > first_row {
            rows.remove(rows.row_count() - 1);
        }
        rows.push(RowData {
            kind: 8,
            title: "Recommended".into(),
            subtitle: "Based on what\u{2019}s in this playlist".into(),
            ..RowData::default()
        });
        let mut ids = Vec::new();
        let mut places = Vec::new();
        for item in &songs {
            let data = self.item_data(item, next_id, 64, Target::Page { generation, id: next_id });
            places.push(Place::Extra(rows.row_count()));
            rows.push(RowData {
                kind: 1,
                style: 3,
                item: data,
                ..RowData::default()
            });
            ids.push(next_id);
            next_id += 1;
        }
        if !loading && songs.is_empty() {
            rows.push(RowData {
                kind: 5,
                title: "No suggestions right now. Try Refresh.".into(),
                ..RowData::default()
            });
        }
        rows.push(RowData {
            kind: 9,
            more: loading,
            ..RowData::default()
        });
        {
            let mut page = self.page.borrow_mut();
            if let Some(rec) = page.recommend.as_mut() {
                rec.shown.extend(songs.iter().filter_map(|s| s.video_id.clone()));
                rec.ids = ids;
            }
            page.items.extend(songs);
            page.places.extend(places);
        }
        self.show_mix_rows();
    }

    /// The Add button on a suggestion: into the playlist, and out of the
    /// section.
    pub fn add_recommended(self: &Rc<Self>, id: usize) {
        let (item, playlist) = {
            let page = self.page.borrow();
            let Some(item) = page.items.get(id).cloned() else { return };
            let playlist = page.header.as_ref().filter(|h| h.editable).and_then(|h| h.playlist_id.clone());
            (item, playlist)
        };
        let (Some(video_id), Some(playlist)) = (item.video_id.clone(), playlist) else {
            return;
        };
        // Out at once, so it cannot be added twice.
        {
            let mut page = self.page.borrow_mut();
            if let Some(Place::Extra(row)) = page.places.get(id) {
                let row = *row;
                page.rows.remove(row);
                page.places[id] = Place::Gone;
                // Rows under it move up one.
                for place in page.places.iter_mut() {
                    if let Place::Extra(r) = place {
                        if *r > row {
                            *r -= 1;
                        }
                    }
                }
                if let Some(rec) = page.recommend.as_mut() {
                    rec.ids.retain(|i| *i != id);
                }
            }
        }
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.add_to_playlist(&playlist, &video_id);
            // The playlist lists the song once it is opened again. Until
            // then the page stays as it is, scrolled where it was.
            on_ui(move |app| match result {
                Ok(()) => app.toast(format!("Added {} to this playlist", item.title)),
                Err(message) => app.toast(message),
            });
        });
    }

    /// Fetches the sound of suggestions with no tempo and key yet, one at a
    /// time, for the analyzer. Suggestions are not on this computer, so
    /// without this they would have none until played.
    pub fn analyze_recommended(&self, video_ids: Vec<String>) {
        let Some(resolver) = self.resolver.clone() else {
            return;
        };
        let wanted: Vec<String> = {
            let mut busy = fetching();
            video_ids
                .into_iter()
                .filter(|id| !crate::library::has_analysis(id) && busy.insert(id.clone()))
                .collect()
        };
        if wanted.is_empty() {
            return;
        }
        let _ = std::thread::Builder::new().name("recommend".into()).spawn(move || {
            for id in wanted {
                let bytes = resolver
                    .resolve(&id)
                    .and_then(|stream| crate::audio::fetch_all(&stream.url, &stream.headers));
                match bytes {
                    Ok(bytes) => crate::analysis::submit(&id, &bytes),
                    Err(message) => eprintln!("recommended {id}: {message}"),
                }
                fetching().remove(&id);
                // Waiting turns to queued, or to the analysis itself.
                on_ui(|app| app.refresh_mix_rows());
            }
        });
    }
}
