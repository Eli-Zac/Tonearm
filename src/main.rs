#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod decode;
mod images;
mod innertube;
mod lyrics;
#[cfg(windows)]
mod media_keys;
mod motion;
mod pool;
mod resolver;
mod services;
mod store;
mod update;

slint::include_modules!();

use innertube::model::{Item, Kind, Page};
use innertube::Client;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SLOW_TICK: Duration = Duration::from_millis(250);
const FAST_TICK: Duration = Duration::from_millis(33);

#[derive(Clone)]
enum Route {
    Home,
    Explore,
    Library,
    Browse { id: String, params: Option<String> },
    Search(String),
}

/// Where a thumbnail goes once it has loaded.
#[derive(Clone, Copy)]
enum Target {
    Page { generation: u64, id: usize },
    Queue { generation: u64, index: usize },
    Track { generation: u64 },
}

enum Place {
    Row(usize),
    Card(Rc<VecModel<ItemData>>, usize),
}

#[derive(Default)]
struct PageState {
    generation: u64,
    items: Vec<Item>,
    places: Vec<Place>,
    rows: Rc<VecModel<RowData>>,
    continuation: Option<String>,
    loading_more: bool,
    /// On search and home, a song click starts that song's radio. On an
    /// album or playlist it queues the page instead.
    radio_on_click: bool,
}

#[derive(Default)]
struct Player {
    queue: Vec<Item>,
    index: usize,
    generation: u64,
    queue_generation: u64,
    duration: f32,
    liked: bool,
    model: Rc<VecModel<ItemData>>,
    extending: bool,
}

#[derive(Default)]
struct LyricsState {
    /// The player generation these lyrics belong to.
    generation: u64,
    synced: bool,
    /// Start, end, vertical offset and line number of every focusable row.
    rows: Vec<(i32, i32, f32, i32)>,
}

/// How much of the current track has really played. Drives scrobbling.
#[derive(Default)]
struct Listen {
    generation: u64,
    played: Duration,
    last: Option<Instant>,
    scrobbled: bool,
    was_playing: bool,
}

struct Cover {
    generation: u64,
    frames: Vec<motion::Frame>,
    index: usize,
    due: Instant,
}

struct App {
    ui: AppWindow,
    client: Arc<Client>,
    api: pool::Pool,
    art: pool::Pool,
    http: ureq::Agent,
    resolver: Option<Arc<dyn resolver::Resolver>>,
    audio: audio::Audio,
    cache: RefCell<images::Cache>,
    pending: RefCell<HashMap<String, Vec<Target>>>,
    history: RefCell<Vec<Route>>,
    page: RefCell<PageState>,
    player: RefCell<Player>,
    lyrics: RefCell<LyricsState>,
    cover: RefCell<Option<Cover>>,
    clock: Cell<(u64, Instant)>,
    timer: Timer,
    fast: Cell<bool>,
    services: services::Services,
    settings: RefCell<services::Settings>,
    lastfm_token: RefCell<String>,
    listen: RefCell<Listen>,
    toast_timer: Timer,
    release: RefCell<Option<update::Release>>,
    #[cfg(windows)]
    keys: RefCell<Option<media_keys::Keys>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Runs `f` on the UI thread with the app. Safe to call from any thread.
fn on_ui(f: impl FnOnce(&Rc<App>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        let app = APP.with(|slot| slot.borrow().clone());
        if let Some(app) = app {
            f(&app);
        }
    });
}

fn clock_text(seconds: f32) -> SharedString {
    let total = seconds.max(0.0) as u32;
    format!("{}:{:02}", total / 60, total % 60).into()
}

/// Search results put the kind first and the length last in the subtitle:
/// "Song • Daft Punk • Discovery • 5:21". Returns the artist and the length.
fn credits(item: &Item) -> (String, f32) {
    let parts: Vec<&str> = item.subtitle.split(" • ").map(str::trim).collect();
    let labels = ["Song", "Video", "Single", "Album", "EP", "Episode"];
    let artist = parts
        .iter()
        .find(|p| !labels.contains(*p))
        .copied()
        .unwrap_or("")
        .to_string();
    let mut seconds = item.duration_secs();
    if seconds == 0.0 {
        if let Some(last) = parts.last().filter(|p| p.contains(':')) {
            seconds = Item {
                duration: last.to_string(),
                ..Item::default()
            }
            .duration_secs();
        }
    }
    (artist, seconds)
}

impl App {
    fn toast(self: &Rc<Self>, message: impl Into<SharedString>) {
        self.ui.set_toast(message.into());
        let weak = Rc::downgrade(self);
        self.toast_timer
            .start(TimerMode::SingleShot, Duration::from_secs(5), move || {
                if let Some(app) = weak.upgrade() {
                    app.ui.set_toast("".into());
                }
            });
    }

    // ---------------------------------------------------------- thumbnails

    fn thumb(self: &Rc<Self>, url: &str, px: u32, round: bool, target: Target) -> Option<Image> {
        if url.is_empty() {
            return None;
        }
        let key = images::Cache::key(url, px, round);
        if let Some(pixels) = self.cache.borrow().get(&key) {
            return Some(Image::from_rgba8(pixels));
        }
        let mut pending = self.pending.borrow_mut();
        if let Some(waiting) = pending.get_mut(&key) {
            waiting.push(target);
            return None;
        }
        pending.insert(key.clone(), vec![target]);
        let (http, url) = (self.http.clone(), url.to_string());
        self.art.run(move || {
            let pixels = images::fetch(&http, &url, px, round);
            on_ui(move |app| {
                let targets = app.pending.borrow_mut().remove(&key).unwrap_or_default();
                let Some(pixels) = pixels else { return };
                app.cache.borrow_mut().put(key, pixels.clone());
                for target in targets {
                    app.place_thumb(target, Image::from_rgba8(pixels.clone()));
                }
            });
        });
        None
    }

    fn place_thumb(&self, target: Target, image: Image) {
        let set = |model: &VecModel<ItemData>, index: usize| {
            if let Some(mut data) = model.row_data(index) {
                data.art = image.clone();
                data.has_art = true;
                model.set_row_data(index, data);
            }
        };
        match target {
            Target::Page { generation, id } => {
                let page = self.page.borrow();
                if page.generation != generation {
                    return;
                }
                match page.places.get(id) {
                    Some(Place::Row(row)) => {
                        if let Some(mut data) = page.rows.row_data(*row) {
                            data.item.art = image;
                            data.item.has_art = true;
                            page.rows.set_row_data(*row, data);
                        }
                    }
                    Some(Place::Card(model, index)) => set(model, *index),
                    None => {}
                }
            }
            Target::Queue { generation, index } => {
                let player = self.player.borrow();
                if player.queue_generation == generation {
                    set(&player.model, index);
                }
            }
            Target::Track { generation } => {
                if self.player.borrow().generation == generation && self.cover.borrow().is_none() {
                    self.set_tint(&image);
                    self.ui.set_track_art(image);
                    self.ui.set_track_has_art(true);
                }
            }
        }
    }

    /// Tints the player and side panel with the cover's average colour,
    /// pulled toward a mid brightness so text stays readable on top.
    fn set_tint(&self, image: &Image) {
        let Some(pixels) = image.to_rgba8() else {
            return;
        };
        let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
        for pixel in pixels.as_slice().iter().step_by(7) {
            r += pixel.r as u32;
            g += pixel.g as u32;
            b += pixel.b as u32;
            n += 1;
        }
        if n == 0 {
            return;
        }
        let (r, g, b) = ((r / n) as f32, (g / n) as f32, (b / n) as f32);
        let peak = r.max(g).max(b).max(1.0);
        let scale = 150.0 / peak;
        let channel = |v: f32| (v * scale).clamp(0.0, 255.0) as u8;
        self.ui.set_tint(slint::Color::from_rgb_u8(
            channel(r),
            channel(g),
            channel(b),
        ));
    }

    fn item_data(self: &Rc<Self>, item: &Item, id: usize, px: u32, target: Target) -> ItemData {
        let art = self.thumb(&item.thumb, px, item.kind == Kind::Artist, target);
        ItemData {
            id: id as i32,
            title: item.title.as_str().into(),
            subtitle: item.subtitle.as_str().into(),
            duration: item.duration.as_str().into(),
            has_art: art.is_some(),
            art: art.unwrap_or_default(),
            round: item.kind == Kind::Artist,
        }
    }

    // --------------------------------------------------------------- pages

    fn open(self: &Rc<Self>, route: Route, push: bool) {
        if matches!(route, Route::Library) && !self.client.signed_in() {
            self.ui.set_nav(3);
            self.ui
                .set_settings_note("Sign in to open your library.".into());
            return;
        }
        if push {
            self.history.borrow_mut().push(route.clone());
        }
        let generation = {
            let mut page = self.page.borrow_mut();
            *page = PageState {
                generation: page.generation + 1,
                radio_on_click: !matches!(route, Route::Browse { .. }),
                ..PageState::default()
            };
            self.ui.set_rows(ModelRc::from(page.rows.clone()));
            page.generation
        };
        self.ui.set_page_title("".into());
        self.ui.set_page_subtitle("".into());
        self.ui.set_page_playable(false);
        self.ui.set_loading(true);
        self.ui.set_can_back(self.history.borrow().len() > 1);
        self.ui.set_nav(match route {
            Route::Home => 0,
            Route::Explore => 1,
            Route::Library => 2,
            _ => -1,
        });
        let client = self.client.clone();
        self.api.run(move || {
            let result = match &route {
                Route::Home => client.browse("FEmusic_home", None),
                Route::Explore => client.browse("FEmusic_explore", None),
                Route::Library => client.browse("FEmusic_library_landing", None),
                Route::Browse { id, params } => client.browse(id, params.as_deref()),
                Route::Search(query) => client.search(query),
            };
            on_ui(move |app| app.show(generation, result));
        });
    }

    fn show(self: &Rc<Self>, generation: u64, result: Result<Page, String>) {
        if self.page.borrow().generation != generation {
            return;
        }
        self.ui.set_loading(false);
        self.page.borrow_mut().loading_more = false;
        match result {
            Ok(page) => {
                if !page.title.is_empty() {
                    self.ui.set_page_title(page.title.as_str().into());
                    self.ui.set_page_subtitle(page.subtitle.as_str().into());
                }
                if page.shelves.is_empty() && self.page.borrow().items.is_empty() {
                    self.toast("Nothing here yet");
                }
                self.append(page);
            }
            Err(message) => self.toast(message),
        }
    }

    fn append(self: &Rc<Self>, new: Page) {
        let (rows, generation, mut next_id) = {
            let page = self.page.borrow();
            (page.rows.clone(), page.generation, page.items.len())
        };
        let last = rows.row_count().wrapping_sub(1);
        if rows.row_data(last).is_some_and(|r| r.kind == 3) {
            rows.remove(last);
        }
        let mut places = Vec::new();
        let mut items = Vec::new();
        for shelf in new.shelves {
            if !shelf.title.is_empty() {
                rows.push(RowData {
                    kind: 0,
                    title: shelf.title.as_str().into(),
                    ..RowData::default()
                });
            }
            if shelf.carousel {
                let cards = Rc::new(VecModel::default());
                for (index, item) in shelf.items.into_iter().enumerate() {
                    cards.push(self.item_data(
                        &item,
                        next_id,
                        192,
                        Target::Page {
                            generation,
                            id: next_id,
                        },
                    ));
                    places.push(Place::Card(cards.clone(), index));
                    items.push(item);
                    next_id += 1;
                }
                rows.push(RowData {
                    kind: 2,
                    cards: ModelRc::from(cards),
                    ..RowData::default()
                });
            } else {
                for item in shelf.items {
                    let data = self.item_data(
                        &item,
                        next_id,
                        64,
                        Target::Page {
                            generation,
                            id: next_id,
                        },
                    );
                    places.push(Place::Row(rows.row_count()));
                    rows.push(RowData {
                        kind: 1,
                        item: data,
                        ..RowData::default()
                    });
                    items.push(item);
                    next_id += 1;
                }
            }
        }
        if new.continuation.is_some() {
            rows.push(RowData {
                kind: 3,
                ..RowData::default()
            });
        }
        let mut page = self.page.borrow_mut();
        page.places.extend(places);
        page.items.extend(items);
        page.continuation = new.continuation;
        let playable = !page.radio_on_click
            && page
                .items
                .iter()
                .zip(&page.places)
                .any(|(i, p)| i.video_id.is_some() && matches!(p, Place::Row(_)));
        drop(page);
        self.ui.set_page_playable(playable);
    }

    fn load_more(self: &Rc<Self>) {
        let (generation, token) = {
            let mut page = self.page.borrow_mut();
            if page.loading_more {
                return;
            }
            let Some(token) = page.continuation.take() else {
                return;
            };
            page.loading_more = true;
            (page.generation, token)
        };
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.more(&token);
            on_ui(move |app| app.show(generation, result));
        });
    }

    fn back(self: &Rc<Self>) {
        if self.ui.get_nav() == 3 {
            // Settings sits on top of the page. Leaving it shows the page again.
            let current = self.history.borrow().last().cloned();
            if let Some(route) = current {
                self.open(route, false);
            }
            return;
        }
        let previous = {
            let mut history = self.history.borrow_mut();
            if history.len() < 2 {
                return;
            }
            history.pop();
            history.last().cloned()
        };
        if let Some(route) = previous {
            self.open(route, false);
        }
    }

    /// Songs on the current page that sit in a vertical list.
    fn page_songs(&self) -> Vec<(usize, Item)> {
        let page = self.page.borrow();
        page.items
            .iter()
            .zip(&page.places)
            .enumerate()
            .filter(|(_, (item, place))| item.video_id.is_some() && matches!(place, Place::Row(_)))
            .map(|(id, (item, _))| (id, item.clone()))
            .collect()
    }

    fn activate(self: &Rc<Self>, id: usize) {
        let (item, radio) = {
            let page = self.page.borrow();
            let Some(item) = page.items.get(id) else {
                return;
            };
            (
                item.clone(),
                page.radio_on_click || matches!(page.places[id], Place::Card(..)),
            )
        };
        if item.video_id.is_some() {
            if radio {
                self.set_queue(vec![item], 0);
            } else {
                let songs = self.page_songs();
                let index = songs.iter().position(|(i, _)| *i == id).unwrap_or(0);
                self.set_queue(songs.into_iter().map(|(_, item)| item).collect(), index);
            }
        } else if let Some(browse_id) = item.browse_id {
            self.open(
                Route::Browse {
                    id: browse_id,
                    params: item.params,
                },
                true,
            );
        } else if let Some(playlist) = item.playlist_id {
            let client = self.client.clone();
            self.api.run(move || {
                let result = client.queue(None, Some(&playlist));
                on_ui(move |app| match result {
                    Ok((items, _)) if !items.is_empty() => app.set_queue(items, 0),
                    Ok(_) => app.toast("That mix is empty"),
                    Err(message) => app.toast(message),
                });
            });
        }
    }

    fn play_page(self: &Rc<Self>, shuffle: bool) {
        let mut songs: Vec<Item> = self
            .page_songs()
            .into_iter()
            .map(|(_, item)| item)
            .collect();
        if songs.is_empty() {
            return;
        }
        if shuffle {
            // Fisher Yates with a xorshift seeded from the clock. Good enough
            // for a playlist and it saves a dependency.
            let mut seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1)
                | 1;
            for i in (1..songs.len()).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                songs.swap(i, (seed % (i as u64 + 1)) as usize);
            }
        }
        self.set_queue(songs, 0);
    }

    // -------------------------------------------------------------- player

    fn set_queue(self: &Rc<Self>, items: Vec<Item>, index: usize) {
        {
            let mut player = self.player.borrow_mut();
            player.queue = Vec::new();
            player.queue_generation += 1;
            player.model = Rc::new(VecModel::default());
            player.extending = false;
            self.ui.set_queue(ModelRc::from(player.model.clone()));
        }
        self.extend_queue(items);
        self.play_index(index);
    }

    fn extend_queue(self: &Rc<Self>, items: Vec<Item>) {
        let (model, generation, start, known) = {
            let player = self.player.borrow();
            let known: Vec<String> = player
                .queue
                .iter()
                .filter_map(|i| i.video_id.clone())
                .collect();
            (
                player.model.clone(),
                player.queue_generation,
                player.queue.len(),
                known,
            )
        };
        let fresh: Vec<Item> = items
            .into_iter()
            .filter(|i| i.video_id.as_ref().is_some_and(|v| !known.contains(v)))
            .collect();
        for (offset, item) in fresh.iter().enumerate() {
            let index = start + offset;
            model.push(self.item_data(item, index, 64, Target::Queue { generation, index }));
        }
        let was_last = {
            let player = self.player.borrow();
            player.index + 1 == player.queue.len()
        };
        self.player.borrow_mut().queue.extend(fresh);
        if was_last {
            self.prefetch_next();
        }
    }

    /// Asks for the radio of the last song and appends it, so music keeps
    /// going when the queue runs out.
    fn extend_with_radio(self: &Rc<Self>) {
        let (generation, video_id) = {
            let mut player = self.player.borrow_mut();
            if player.extending {
                return;
            }
            let Some(id) = player.queue.last().and_then(|i| i.video_id.clone()) else {
                return;
            };
            player.extending = true;
            (player.queue_generation, id)
        };
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.queue(Some(&video_id), None);
            on_ui(move |app| {
                if app.player.borrow().queue_generation != generation {
                    return;
                }
                if let Ok((items, _)) = result {
                    app.extend_queue(items);
                }
                app.player.borrow_mut().extending = false;
            });
        });
    }

    fn play_index(self: &Rc<Self>, index: usize) {
        let (item, generation, near_end) = {
            let mut player = self.player.borrow_mut();
            let Some(item) = player.queue.get(index).cloned() else {
                return;
            };
            player.index = index;
            player.generation += 1;
            player.liked = false;
            player.duration = credits(&item).1;
            (item, player.generation, index + 2 >= player.queue.len())
        };
        let (artist, seconds) = credits(&item);
        let ui = &self.ui;
        ui.set_has_track(true);
        ui.set_track_title(item.title.as_str().into());
        ui.set_track_artist(artist.as_str().into());
        ui.set_queue_index(index as i32);
        ui.set_liked(false);
        ui.set_progress(0.0);
        ui.set_position_text(clock_text(0.0));
        ui.set_duration_text(clock_text(seconds));
        ui.set_playing(false);
        *self.cover.borrow_mut() = None;
        let art = self.thumb(&item.thumb, 128, false, Target::Track { generation });
        if let Some(image) = &art {
            self.set_tint(image);
        }
        ui.set_track_has_art(art.is_some());
        ui.set_track_art(art.unwrap_or_default());
        self.load_cover(&item.title, &artist, generation);
        if ui.get_panel() == 2 {
            self.load_lyrics();
        }
        #[cfg(windows)]
        self.update_keys(&item.title, &artist, seconds);
        if near_end {
            self.extend_with_radio();
        }
        let Some(resolver) = self.resolver.clone() else {
            self.audio.stop();
            self.services.send(services::Event::Stopped);
            ui.set_buffering(false);
            self.toast("Playback is not set up yet. The README explains the missing piece.");
            return;
        };
        ui.set_buffering(true);
        let video_id = item.video_id.unwrap_or_default();
        self.api.run(move || {
            let result = resolver.resolve(&video_id);
            on_ui(move |app| {
                if app.player.borrow().generation != generation {
                    return;
                }
                match result {
                    Ok(stream) => {
                        if stream.duration > 0.0 {
                            app.player.borrow_mut().duration = stream.duration;
                            app.ui.set_duration_text(clock_text(stream.duration));
                        }
                        app.audio.load(stream.url, stream.headers, generation);
                        app.prefetch_next();
                    }
                    Err(message) => {
                        app.ui.set_buffering(false);
                        app.toast(message);
                    }
                }
            });
        });
    }

    /// Resolves the next track in the background so skipping to it, or
    /// reaching it, starts without the yt-dlp wait.
    fn prefetch_next(self: &Rc<Self>) {
        let Some(resolver) = self.resolver.clone() else {
            return;
        };
        let next = {
            let player = self.player.borrow();
            player
                .queue
                .get(player.index + 1)
                .and_then(|item| item.video_id.clone())
        };
        if let Some(video_id) = next {
            self.api.run(move || resolver.prefetch(&video_id));
        }
    }

    fn load_cover(self: &Rc<Self>, title: &str, artist: &str, generation: u64) {
        let Some(url) = motion::lookup(title, artist) else {
            return;
        };
        let http = self.http.clone();
        self.art.run(move || {
            let Some(frames) = motion::fetch(&http, &url) else {
                return;
            };
            on_ui(move |app| {
                if app.player.borrow().generation == generation {
                    *app.cover.borrow_mut() = Some(Cover {
                        generation,
                        frames,
                        index: 0,
                        due: Instant::now(),
                    });
                }
            });
        });
    }

    #[cfg(windows)]
    fn update_keys(self: &Rc<Self>, title: &str, artist: &str, seconds: f32) {
        let mut keys = self.keys.borrow_mut();
        if keys.is_none() {
            *keys = media_keys::Keys::new(self.ui.window(), |key| {
                on_ui(move |app| match key {
                    media_keys::Key::Toggle => app.toggle_play(),
                    media_keys::Key::Next => app.next(),
                    media_keys::Key::Previous => app.previous(),
                })
            });
        }
        if let Some(keys) = keys.as_mut() {
            keys.track(title, artist, seconds);
        }
    }

    fn toggle_play(self: &Rc<Self>) {
        match self.audio.status.state.load(Ordering::SeqCst) {
            audio::PLAYING => self.audio.pause(),
            audio::PAUSED => self.audio.play(),
            audio::LOADING => {}
            _ => {
                let index = self.player.borrow().index;
                self.play_index(index);
            }
        }
    }

    fn next(self: &Rc<Self>) {
        let index = self.player.borrow().index + 1;
        self.play_index(index);
    }

    fn previous(self: &Rc<Self>) {
        let index = self.player.borrow().index;
        if self.audio.status.position_ms.load(Ordering::SeqCst) > 3000 || index == 0 {
            self.audio.seek(0.0);
        } else {
            self.play_index(index - 1);
        }
    }

    fn toggle_like(self: &Rc<Self>) {
        if !self.client.signed_in() {
            self.toast("Sign in to like songs");
            return;
        }
        let (video_id, liked) = {
            let mut player = self.player.borrow_mut();
            let Some(id) = player
                .queue
                .get(player.index)
                .and_then(|i| i.video_id.clone())
            else {
                return;
            };
            player.liked = !player.liked;
            (id, player.liked)
        };
        self.ui.set_liked(liked);
        let client = self.client.clone();
        self.api.run(move || {
            if let Err(message) = client.rate(&video_id, liked) {
                on_ui(move |app| app.toast(message));
            }
        });
    }

    // -------------------------------------------------------------- lyrics

    fn load_lyrics(self: &Rc<Self>) {
        let (item, generation) = {
            let player = self.player.borrow();
            let Some(item) = player.queue.get(player.index).cloned() else {
                return;
            };
            (item, player.generation)
        };
        {
            let mut lyrics = self.lyrics.borrow_mut();
            if lyrics.generation == generation {
                return;
            }
            *lyrics = LyricsState {
                generation,
                ..LyricsState::default()
            };
        }
        self.ui.set_lyrics(ModelRc::default());
        self.ui.set_lyrics_state(1);
        self.ui.set_lyrics_synced(false);
        let client = self.client.clone();
        self.api.run(move || {
            let (artist, seconds) = credits(&item);
            let query = lyrics::Query {
                video_id: item.video_id.as_deref().unwrap_or(""),
                title: &item.title,
                artist: &artist,
                seconds,
            };
            let found = lyrics::fetch(&client, &query);
            on_ui(move |app| app.show_lyrics(generation, found));
        });
    }

    fn show_lyrics(&self, generation: u64, found: Option<lyrics::Lyrics>) {
        let mut state = self.lyrics.borrow_mut();
        if state.generation != generation {
            return;
        }
        let Some(found) = found.filter(|l| !l.lines.is_empty()) else {
            self.ui.set_lyrics_state(3);
            return;
        };
        let (columns, height) = if found.synced { (24, 36.0) } else { (48, 24.0) };
        let rows = VecModel::default();
        let mut y = 0.0;
        let mut previous_end = 0u32;
        for (number, line) in found.lines.iter().enumerate() {
            let number = number as i32;
            if found.synced {
                // A long wait before a line gets the three interlude dots.
                if line.start > previous_end + 5000 {
                    let (start, end) = (previous_end as i32, line.start as i32);
                    state.rows.push((start, end, y, number - 1));
                    rows.push(LyricRow {
                        kind: 2,
                        start,
                        end,
                        line: number - 1,
                        ..LyricRow::default()
                    });
                }
                if number > 0 {
                    rows.push(LyricRow {
                        kind: 1,
                        line: number,
                        ..LyricRow::default()
                    });
                    y += 18.0;
                }
                previous_end = line.end;
            }
            for (text, syllables) in wrap(line, columns) {
                let (start, end) = (line.start as i32, line.end as i32);
                state.rows.push((start, end, y, number));
                y += height;
                let syllables: Vec<SyllableData> = syllables
                    .iter()
                    .map(|s| SyllableData {
                        text: s.text.trim_end().into(),
                        start: s.start as i32,
                        end: s.end as i32,
                        space: s.text.ends_with(' '),
                    })
                    .collect();
                rows.push(LyricRow {
                    text: text.into(),
                    syllables: ModelRc::new(VecModel::from(syllables)),
                    start,
                    end,
                    kind: 0,
                    line: number,
                });
            }
        }
        state.synced = found.synced;
        self.ui.set_lyrics(ModelRc::new(rows));
        self.ui.set_lyrics_synced(found.synced);
        self.ui.set_lyrics_source(found.source.into());
        self.ui.set_lyrics_focus(0.0);
        self.ui.set_lyrics_line(-1);
        self.ui.set_lyrics_state(2);
    }

    // ---------------------------------------------------------------- tick

    fn tick(self: &Rc<Self>) {
        let status = &self.audio.status;
        let state = status.state.load(Ordering::SeqCst);
        let (generation, duration) = {
            let player = self.player.borrow();
            (player.generation, player.duration)
        };
        let ui = &self.ui;
        let mut time = 0u64;
        if status.generation.load(Ordering::SeqCst) == generation {
            let playing = state == audio::PLAYING;
            if ui.get_playing() != playing {
                ui.set_playing(playing);
                #[cfg(windows)]
                if let Some(keys) = self.keys.borrow_mut().as_mut() {
                    keys.playing(playing);
                }
            }
            ui.set_buffering(state == audio::LOADING);
            self.track_listening(generation, playing, duration);
            match state {
                audio::ENDED => {
                    status.state.store(audio::IDLE, Ordering::SeqCst);
                    self.next();
                    return;
                }
                audio::FAILED => {
                    status.state.store(audio::IDLE, Ordering::SeqCst);
                    let message = status.error.lock().unwrap().clone();
                    self.toast(message);
                }
                _ => {}
            }
            // The audio thread reports position ten times a second. Between
            // reports we run our own clock so lyrics move smoothly.
            let reported = status.position_ms.load(Ordering::SeqCst);
            let (base, at) = self.clock.get();
            if reported != base {
                self.clock.set((reported, Instant::now()));
                time = reported;
            } else if playing {
                time = base + (at.elapsed().as_millis() as u64).min(200);
            } else {
                time = base;
            }
            let seconds = time as f32 / 1000.0;
            ui.set_position_text(clock_text(seconds));
            ui.set_progress(if duration > 0.0 {
                (seconds / duration).min(1.0)
            } else {
                0.0
            });
        }

        let lyrics_live = ui.get_panel() == 2 && self.lyrics.borrow().synced;
        if lyrics_live {
            ui.set_lyrics_time(time as i32);
            let lyrics = self.lyrics.borrow();
            let now = time as i32;
            // The focus is the latest row that has started. Rows are in
            // time order, apart from wrapped rows that share one start.
            let started = lyrics
                .rows
                .iter()
                .filter(|row| row.0 <= now)
                .max_by_key(|row| row.0);
            if let Some((_, _, y, line)) = started {
                let first = lyrics
                    .rows
                    .iter()
                    .find(|row| row.3 == *line && row.0 == started.unwrap().0)
                    .map(|row| row.2)
                    .unwrap_or(*y);
                ui.set_lyrics_focus(first);
                ui.set_lyrics_line(*line);
            } else {
                ui.set_lyrics_line(-1);
            }
        }

        let mut animating = false;
        if let Some(cover) = self.cover.borrow_mut().as_mut() {
            if cover.generation == generation {
                animating = true;
                if Instant::now() >= cover.due {
                    let frame = &cover.frames[cover.index];
                    ui.set_track_art(Image::from_rgba8(frame.pixels.clone()));
                    ui.set_track_has_art(true);
                    cover.due = Instant::now() + frame.delay;
                    cover.index = (cover.index + 1) % cover.frames.len();
                }
            }
        }

        // Tick fast only while something on screen is moving.
        let fast = animating || (lyrics_live && state == audio::PLAYING);
        if fast != self.fast.replace(fast) {
            self.timer
                .set_interval(if fast { FAST_TICK } else { SLOW_TICK });
        }
    }

    /// Tells Discord and Last.fm what is going on. A track scrobbles once
    /// it has played for half its length or four minutes, as Last.fm asks.
    fn track_listening(self: &Rc<Self>, generation: u64, playing: bool, duration: f32) {
        use services::Event;
        let mut listen = self.listen.borrow_mut();
        let now = Instant::now();
        if listen.generation != generation {
            if !playing {
                return;
            }
            *listen = Listen {
                generation,
                was_playing: true,
                last: Some(now),
                ..Listen::default()
            };
            let player = self.player.borrow();
            if let Some(item) = player.queue.get(player.index) {
                let (artist, seconds) = credits(item);
                let seconds = if duration > 0.0 { duration } else { seconds };
                self.services.send(Event::Started(services::Track {
                    title: item.title.clone(),
                    artist,
                    seconds,
                    art: item.thumb.clone(),
                }));
            }
            return;
        }
        if playing {
            if let Some(last) = listen.last {
                listen.played += now.duration_since(last).min(Duration::from_secs(1));
            }
            let needed = (duration / 2.0).min(240.0);
            if !listen.scrobbled && duration > 30.0 && listen.played.as_secs_f32() >= needed {
                listen.scrobbled = true;
                self.services.send(Event::Scrobble);
            }
        }
        if playing != listen.was_playing {
            listen.was_playing = playing;
            let position = self.audio.status.position_ms.load(Ordering::SeqCst) as f32 / 1000.0;
            self.services.send(if playing {
                Event::Resumed { position }
            } else {
                Event::Paused
            });
        }
        listen.last = playing.then_some(now);
    }

    // ------------------------------------------------------------ services

    fn show_services(&self) {
        let s = self.settings.borrow();
        self.ui.set_svc_discord(s.discord_app.as_str().into());
        self.ui.set_svc_lastfm_key(s.lastfm_key.as_str().into());
        self.ui
            .set_svc_lastfm_secret(s.lastfm_secret.as_str().into());
        self.ui.set_svc_spotify_id(s.spotify_id.as_str().into());
        self.ui
            .set_svc_spotify_secret(s.spotify_secret.as_str().into());
        self.ui.set_lastfm_user(s.lastfm_user.as_str().into());
    }

    fn apply_services(self: &Rc<Self>, note: &str) {
        let settings = self.settings.borrow().clone();
        let note = match settings.save() {
            Ok(()) => note.to_string(),
            Err(message) => format!("Could not save: {message}"),
        };
        self.services.send(services::Event::Configure(settings));
        self.show_services();
        self.ui.set_services_note(note.into());
    }

    fn save_services(self: &Rc<Self>) {
        {
            let mut s = self.settings.borrow_mut();
            let (key, secret) = (
                self.ui.get_svc_lastfm_key().trim().to_string(),
                self.ui.get_svc_lastfm_secret().trim().to_string(),
            );
            if key != s.lastfm_key || secret != s.lastfm_secret {
                // New keys cannot reuse the old sign in.
                s.lastfm_session.clear();
                s.lastfm_user.clear();
            }
            s.lastfm_key = key;
            s.lastfm_secret = secret;
            s.discord_app = self.ui.get_svc_discord().trim().to_string();
            s.spotify_id = self.ui.get_svc_spotify_id().trim().to_string();
            s.spotify_secret = self.ui.get_svc_spotify_secret().trim().to_string();
        }
        self.apply_services("Saved");
    }

    fn lastfm_connect(self: &Rc<Self>) {
        self.save_services();
        let (key, secret) = {
            let s = self.settings.borrow();
            (s.lastfm_key.clone(), s.lastfm_secret.clone())
        };
        if key.is_empty() || secret.is_empty() {
            self.ui
                .set_services_note("Enter your Last.fm API key and shared secret first.".into());
            return;
        }
        self.api.run(move || {
            let result = services::lastfm_begin(&key, &secret);
            on_ui(move |app| match result {
                Ok((token, url)) => {
                    *app.lastfm_token.borrow_mut() = token;
                    services::open_browser(&url);
                    app.ui.set_services_note(
                        "Approve Tonearm in the browser page that opened, then press Finish."
                            .into(),
                    );
                }
                Err(message) => app.ui.set_services_note(message.into()),
            });
        });
    }

    fn lastfm_finish(self: &Rc<Self>) {
        let token = self.lastfm_token.borrow().clone();
        if token.is_empty() {
            self.ui
                .set_services_note("Press Connect Last.fm first.".into());
            return;
        }
        let (key, secret) = {
            let s = self.settings.borrow();
            (s.lastfm_key.clone(), s.lastfm_secret.clone())
        };
        self.api.run(move || {
            let result = services::lastfm_finish(&key, &secret, &token);
            on_ui(move |app| match result {
                Ok((session, user)) => {
                    {
                        let mut s = app.settings.borrow_mut();
                        s.lastfm_session = session;
                        s.lastfm_user = user;
                    }
                    app.lastfm_token.borrow_mut().clear();
                    app.apply_services("Last.fm is connected");
                }
                Err(message) => app.ui.set_services_note(message.into()),
            });
        });
    }

    // ------------------------------------------------------------- sign in

    fn check_account(self: &Rc<Self>) {
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.account_name();
            on_ui(move |app| match result {
                Ok(Some(name)) => {
                    app.ui.set_signed_in(true);
                    app.ui.set_account_name(name.into());
                    app.ui.set_settings_note("".into());
                    let current = app.history.borrow().last().cloned();
                    if let (Some(route), true) = (current, app.ui.get_nav() != 3) {
                        app.open(route, false);
                    }
                }
                Ok(None) => {
                    app.client.set_cookie(None);
                    app.ui.set_signed_in(false);
                    app.ui.set_settings_note(
                        "YouTube Music did not accept that cookie. Copy a fresh one and try again."
                            .into(),
                    );
                }
                Err(message) => app.ui.set_settings_note(message.into()),
            });
        });
    }

    fn save_cookie(self: &Rc<Self>, raw: &str) {
        let Some(cookie) = innertube::normalize_cookie(raw) else {
            self.ui.set_settings_note(
                "That is not a YouTube Music cookie. It must contain SAPISID.".into(),
            );
            return;
        };
        self.client.set_cookie(Some(&cookie));
        if let Err(message) = store::save("session", &cookie) {
            self.toast(format!("Could not save the sign in: {message}"));
        }
        self.ui.set_settings_note("Checking".into());
        self.check_account();
    }

    /// Asks GitHub for a newer release. A quiet check only speaks up when
    /// there is one, so a failed check at start up stays out of the way.
    fn check_update(self: &Rc<Self>, quiet: bool) {
        if self.ui.get_update_busy() {
            return;
        }
        self.ui.set_update_busy(true);
        if !quiet {
            self.ui.set_update_note("".into());
        }
        let http = self.http.clone();
        self.api.run(move || {
            let result = update::check(&http);
            on_ui(move |app| {
                app.ui.set_update_busy(false);
                match result {
                    Ok(Some(release)) => {
                        let note = format!("Tonearm {} is ready to install.", release.version);
                        app.ui.set_update_note(note.as_str().into());
                        app.ui.set_update_available(true);
                        *app.release.borrow_mut() = Some(release);
                        if quiet {
                            app.toast(format!("{note} Open Settings to update."));
                        }
                    }
                    Ok(None) => {
                        app.ui.set_update_available(false);
                        if !quiet {
                            app.ui
                                .set_update_note("You have the latest version.".into());
                        }
                    }
                    Err(message) => {
                        if !quiet {
                            app.ui.set_update_note(message.into());
                        }
                    }
                }
            });
        });
    }

    fn install_update(self: &Rc<Self>) {
        let Some(release) = self.release.borrow().clone() else {
            return;
        };
        if self.ui.get_update_busy() {
            return;
        }
        self.ui.set_update_busy(true);
        self.ui
            .set_update_note(format!("Downloading Tonearm {}", release.version).into());
        let http = self.http.clone();
        self.api.run(move || {
            let result = update::install(&http, &release);
            on_ui(move |app| match result {
                Ok(()) => {
                    app.ui.set_update_note("Restarting".into());
                    app.audio.stop();
                    update::restart();
                }
                Err(message) => {
                    app.ui.set_update_busy(false);
                    app.ui.set_update_note(message.into());
                }
            });
        });
    }

    fn sign_out(self: &Rc<Self>) {
        store::clear("session");
        self.client.set_cookie(None);
        self.ui.set_signed_in(false);
        self.ui.set_account_name("".into());
    }
}

/// Breaks a lyric line into rows of at most `columns` characters. Breaks
/// fall only between words, so a syllable sweep never splits a word.
fn wrap(line: &lyrics::Line, columns: usize) -> Vec<(String, Vec<lyrics::Syllable>)> {
    let mut rows: Vec<(String, Vec<lyrics::Syllable>)> = vec![Default::default()];
    if line.syllables.is_empty() {
        for word in line.text.split_whitespace() {
            let row = rows.last_mut().unwrap();
            if !row.0.is_empty() && row.0.chars().count() + word.chars().count() + 1 > columns {
                rows.push((word.to_string(), Vec::new()));
            } else {
                if !row.0.is_empty() {
                    row.0.push(' ');
                }
                row.0.push_str(word);
            }
        }
        return rows;
    }
    let mut word: Vec<lyrics::Syllable> = Vec::new();
    let count = line.syllables.len();
    for (index, syllable) in line.syllables.iter().enumerate() {
        word.push(syllable.clone());
        if !syllable.text.ends_with(' ') && index + 1 < count {
            continue;
        }
        let length: usize = word.iter().map(|s| s.text.chars().count()).sum();
        if !rows.last().unwrap().0.is_empty()
            && rows.last().unwrap().0.chars().count() + length > columns
        {
            rows.push(Default::default());
        }
        let row = rows.last_mut().unwrap();
        for s in word.drain(..) {
            row.0.push_str(&s.text);
            row.1.push(s);
        }
    }
    rows
}

#[cfg(windows)]
fn set_font(ui: &AppWindow) {
    ui.global::<Theme>()
        .set_font("Segoe UI Variable Display".into());
}

#[cfg(not(windows))]
fn set_font(_ui: &AppWindow) {}

/// `tonearm --probe search <query>` and friends print parsed results, so we
/// can check the parsers against the live service without the UI.
fn probe(args: &[String]) {
    let client = Client::new();
    if let Some(cookie) = store::load("session") {
        client.set_cookie(Some(&cookie));
    }
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    let page = match arg(0) {
        "search" => client.search(arg(1)),
        "browse" => client.browse(arg(1), None),
        "queue" => client.queue(Some(arg(1)), None).map(|(items, _)| Page {
            shelves: vec![innertube::model::Shelf {
                title: "Queue".into(),
                carousel: false,
                items,
            }],
            ..Page::default()
        }),
        "play" => {
            let Some(resolver) = resolver::default() else {
                return println!("error: no resolver");
            };
            let started = Instant::now();
            let stream = match resolver.resolve(arg(1)) {
                Ok(stream) => stream,
                Err(message) => return println!("error: {message}"),
            };
            println!("resolved in {:?}", started.elapsed());
            let audio = audio::Audio::new();
            audio.volume(0.0);
            audio.load(stream.url, stream.headers, 1);
            for _ in 0..16 {
                std::thread::sleep(Duration::from_millis(500));
                let status = &audio.status;
                println!(
                    "{:>5?} state {} position {} ms {}",
                    started.elapsed().as_millis(),
                    status.state.load(Ordering::SeqCst),
                    status.position_ms.load(Ordering::SeqCst),
                    status.error.lock().unwrap()
                );
            }
            audio.seek(150.0);
            std::thread::sleep(Duration::from_secs(2));
            println!(
                "after seek to 150 s: state {} position {} ms",
                audio.status.state.load(Ordering::SeqCst),
                audio.status.position_ms.load(Ordering::SeqCst)
            );
            return;
        }
        "decode" => {
            // `--probe decode <file.m4a> <out.raw>` writes interleaved i16.
            let file = match std::fs::File::open(arg(1)) {
                Ok(f) => f,
                Err(e) => return println!("error: {e}"),
            };
            let track = match decode::Track::open(Box::new(file), arg(3).parse().unwrap_or(0)) {
                Ok(t) => t,
                Err(e) => return println!("error: {e}"),
            };
            use rodio::Source;
            println!("channels {} rate {}", track.channels(), track.sample_rate());
            // What the output device gets, after rodio's own conversion.
            let rate = track.sample_rate();
            let samples: Vec<i16> =
                rodio::source::UniformSourceIterator::<_, i16>::new(track, 2, rate).collect();
            let clipped = samples.iter().filter(|s| s.unsigned_abs() >= 32767).count();
            println!("samples {} clipped {}", samples.len(), clipped);
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            let _ = std::fs::write(arg(2), bytes);
            return;
        }
        "stream" => {
            let Some(resolver) = resolver::default() else {
                return println!("error: no resolver");
            };
            match resolver.resolve(arg(1)) {
                Ok(stream) => {
                    println!("duration {}", stream.duration);
                    println!("headers {:?}", stream.headers);
                    println!("url {}", stream.url);
                    let mut request = innertube::agent(Duration::from_secs(15))
                        .get(&stream.url)
                        .set("Range", "bytes=2000000-2000063");
                    for (key, value) in &stream.headers {
                        request = request.set(key, value);
                    }
                    match request.call() {
                        Ok(response) => {
                            println!(
                                "http {} {:?}",
                                response.status(),
                                response.header("Content-Type")
                            );
                            let mut head = Vec::new();
                            let _ =
                                std::io::Read::read_to_end(&mut response.into_reader(), &mut head);
                            println!("head {:?}", String::from_utf8_lossy(&head));
                        }
                        Err(e) => println!("fetch error: {e}"),
                    }
                }
                Err(message) => println!("error: {message}"),
            }
            return;
        }
        "lyrics" => {
            let query = lyrics::Query {
                video_id: arg(4),
                title: arg(1),
                artist: arg(2),
                seconds: arg(3).parse().unwrap_or(0.0),
            };
            match lyrics::fetch(&client, &query) {
                Some(found) => {
                    println!(
                        "source {} synced {} lines {}",
                        found.source,
                        found.synced,
                        found.lines.len()
                    );
                    for line in found.lines.iter().take(6) {
                        println!(
                            "{:>7} {:>7} {} [{} syllables]",
                            line.start,
                            line.end,
                            line.text,
                            line.syllables.len()
                        );
                    }
                }
                None => println!("no lyrics"),
            }
            return;
        }
        "thumb" => {
            let http = innertube::agent(Duration::from_secs(15));
            let started = Instant::now();
            match images::fetch(&http, arg(1), 192, false) {
                Some(pixels) => println!(
                    "{}x{} in {:?}",
                    pixels.width(),
                    pixels.height(),
                    started.elapsed()
                ),
                None => println!("thumbnail failed"),
            }
            return;
        }
        _ => Err("usage: --probe search|browse|queue|lyrics|thumb ...".into()),
    };
    match page {
        Ok(page) => {
            println!(
                "title {:?} subtitle {:?} more {}",
                page.title,
                page.subtitle,
                page.continuation.is_some()
            );
            for shelf in &page.shelves {
                println!(
                    "== {} ({} items, carousel {})",
                    shelf.title,
                    shelf.items.len(),
                    shelf.carousel
                );
                for item in shelf.items.iter().take(4) {
                    let id = item
                        .video_id
                        .as_ref()
                        .or(item.browse_id.as_ref())
                        .or(item.playlist_id.as_ref());
                    println!(
                        "   {:?} {} | {} | {} | {:?} | art {}",
                        item.kind,
                        item.title,
                        item.subtitle,
                        item.duration,
                        id,
                        !item.thumb.is_empty()
                    );
                }
            }
        }
        Err(message) => println!("error: {message}"),
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--probe") {
        probe(&args[1..]);
        return Ok(());
    }

    update::clean_up();
    let ui = AppWindow::new()?;
    set_font(&ui);
    ui.set_app_version(update::VERSION.trim_start_matches('v').into());
    let client = Arc::new(Client::new());
    let signed_in = store::load("session").is_some_and(|cookie| client.set_cookie(Some(&cookie)));

    let settings = services::Settings::load();
    let app = Rc::new(App {
        services: services::Services::start(settings.clone()),
        settings: RefCell::new(settings),
        lastfm_token: RefCell::default(),
        listen: RefCell::default(),
        ui,
        client,
        api: pool::Pool::new("api", 3),
        art: pool::Pool::new("art", 2),
        http: innertube::agent(Duration::from_secs(15)),
        resolver: resolver::default().map(Arc::from),
        audio: audio::Audio::new(),
        cache: RefCell::default(),
        pending: RefCell::default(),
        history: RefCell::default(),
        page: RefCell::default(),
        player: RefCell::default(),
        lyrics: RefCell::default(),
        cover: RefCell::new(None),
        clock: Cell::new((0, Instant::now())),
        timer: Timer::default(),
        fast: Cell::new(false),
        toast_timer: Timer::default(),
        release: RefCell::new(None),
        #[cfg(windows)]
        keys: RefCell::new(None),
    });
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));

    // Each callback holds a weak handle, so closing the window frees the app.
    macro_rules! on {
        ($setter:ident, |$app:ident $(, $arg:ident)*| $body:expr) => {{
            let weak = Rc::downgrade(&app);
            app.ui.$setter(move |$($arg),*| {
                if let Some($app) = weak.upgrade() {
                    $body
                }
            });
        }};
    }
    on!(on_navigate, |app, index| match index {
        0 => app.open(Route::Home, true),
        1 => app.open(Route::Explore, true),
        2 => app.open(Route::Library, true),
        _ => app.ui.set_nav(3),
    });
    on!(on_back, |app| app.back());
    on!(on_search, |app, query| {
        let query = query.trim().to_string();
        if !query.is_empty() {
            app.open(Route::Search(query), true);
        }
    });
    on!(on_item_activated, |app, id| app.activate(id as usize));
    on!(on_queue_activated, |app, index| app
        .play_index(index as usize));
    on!(on_load_more, |app| app.load_more());
    on!(on_play_page, |app, shuffle| app.play_page(shuffle));
    on!(on_toggle_play, |app| app.toggle_play());
    on!(on_next, |app| app.next());
    on!(on_previous, |app| app.previous());
    on!(on_seek, |app, fraction| {
        let duration = app.player.borrow().duration;
        app.audio.seek(fraction * duration);
    });
    on!(on_set_volume, |app, volume| {
        app.ui.set_volume(volume);
        app.audio.volume(volume * volume);
    });
    on!(on_toggle_like, |app| app.toggle_like());
    on!(on_panel_changed, |app, panel| {
        if panel == 2 {
            app.load_lyrics();
        }
    });
    on!(on_lyrics_seek, |app, ms| app.audio.seek(ms as f32 / 1000.0));
    on!(on_save_cookie, |app, text| app.save_cookie(&text));
    on!(on_sign_out, |app| app.sign_out());
    on!(on_save_services, |app| app.save_services());
    on!(on_lastfm_connect, |app| app.lastfm_connect());
    on!(on_lastfm_finish, |app| app.lastfm_finish());
    on!(on_check_update, |app| app.check_update(false));
    on!(on_install_update, |app| app.install_update());
    app.show_services();

    let weak = Rc::downgrade(&app);
    app.timer.start(TimerMode::Repeated, SLOW_TICK, move || {
        if let Some(app) = weak.upgrade() {
            app.tick();
        }
    });

    app.open(Route::Home, true);
    if signed_in {
        app.check_account();
    }
    app.check_update(true);
    let result = app.ui.run();
    APP.with(|slot| slot.borrow_mut().take());
    update::relaunch_if_updated();
    result
}
