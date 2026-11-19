#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod animated_art;
mod audio;
mod blur;
mod canvas;
mod decode;
mod detail;
mod history;
mod home;
mod images;
mod innertube;
mod library;
mod lyrics;
#[cfg(windows)]
mod media_keys;
mod menu;
mod mix;
mod mixing;
mod recommend;
mod transition;
#[cfg(windows)]
mod shell;
#[cfg(windows)]
mod single;
#[cfg(windows)]
mod taskbar;
mod motion;
mod offline;
mod pool;
mod resolver;
mod services;
mod session;
mod store;
mod update;
mod wheel;

slint::include_modules!();

use innertube::model::{Header, Item, Kind, Page, PageKind, Trend};
use innertube::Client;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SLOW_TICK: Duration = Duration::from_millis(250);
const FAST_TICK: Duration = Duration::from_millis(33);
/// How often the playing bars move. Each move redraws the whole window,
/// so they step like a level meter instead of running at the screen's rate.
const BEAT_TICK: Duration = Duration::from_millis(66);
/// Links got ahead of a click at once. More would only crowd yt-dlp.
const WARM_AT_ONCE: usize = 2;
/// How often the playback session is saved while a song plays, so a
/// crash loses little of your place.
const SESSION_EVERY: Duration = Duration::from_secs(10);

#[derive(Clone)]
enum Route {
    Home,
    Explore,
    Library,
    Browse { id: String, params: Option<String> },
    /// A search and its results tab: 0 all, then songs, albums, artists,
    /// playlists and podcasts.
    Search(String, usize),
    /// Songs saved on this computer. Opens with no connection.
    Downloads,
}

/// Where a thumbnail goes once it has loaded.
#[derive(Clone, Copy)]
enum Target {
    Page { generation: u64, id: usize },
    Queue { generation: u64, index: usize },
    Track { generation: u64 },
    Suggest { generation: u64, row: usize },
    /// The art at the top of a detail page.
    Hero { generation: u64 },
    /// The signed in account's picture.
    Account,
    /// The account's picture at the top of the profile page.
    ProfilePhoto,
    /// A card under Played lately on the profile page.
    Profile { generation: u64, index: usize },
}

/// What the profile page shows, kept to act on clicks.
#[derive(Default)]
struct Profile {
    /// Counts up each time the page is built, so art for an older one is
    /// dropped.
    generation: u64,
    played: Vec<Item>,
    artists: Vec<String>,
    model: Option<Rc<VecModel<ItemData>>>,
}

/// What a row under the search bar does when picked.
#[derive(Clone)]
enum Pick {
    Query(String),
    Item(Item),
}

/// The list under the search bar.
#[derive(Default)]
struct Suggest {
    /// Bumped on every keystroke. Replies for older ones only fill the cache.
    generation: u64,
    picks: Vec<Pick>,
    rows: Rc<VecModel<SuggestData>>,
    /// Replies by lowercased query, so deleting a letter shows results at once.
    /// Each holds the suggested searches and the real search results, so
    /// the list shows the same items as the results page, and pressing
    /// Enter can open that page without asking again.
    cache: HashMap<String, Suggested>,
}

#[derive(Clone)]
struct Suggested {
    queries: Vec<String>,
    page: Page,
    at: Instant,
}

/// Where recent searches are kept, and how many.
const RECENT_SEARCHES: &str = "searches";
const RECENT_SEARCHES_KEPT: usize = 8;

/// The recent searches saved on this computer, newest first.
fn load_recent_searches() -> Vec<String> {
    store::load(RECENT_SEARCHES)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(RECENT_SEARCHES_KEPT)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// How long typing has to pause before we ask for suggestions.
const SUGGEST_DELAY: Duration = Duration::from_millis(140);
/// How long fetched results stay good enough to show again.
const SUGGEST_FRESH: Duration = Duration::from_secs(300);

enum Place {
    Row(usize),
    Card(Rc<VecModel<ItemData>>, usize),
    /// A row after the track list, such as a suggestion under a playlist.
    /// Not one of the page's songs: a click plays its radio.
    Extra(usize),
    /// A row taken off the page.
    Gone,
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
    /// What was searched for, on a results page. Results get tailored to
    /// the listening history.
    query: Option<String>,
    /// The hero of an artist, album or playlist page.
    header: Option<Header>,
    /// What the "Show all" buttons open, by `RowData.more_id`.
    mores: Vec<(String, Option<String>)>,
    /// Tracks numbered so far, so a playlist's next page counts on.
    numbered: usize,
    /// What the track list is sorted by, from its column titles: 0 its own
    /// order, 1 title, 2 album; and whether it runs backwards.
    sort: (i32, bool),
    /// "Recommended" under a playlist of the user's own.
    recommend: Option<recommend::Recommend>,
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
    /// The rows of `model` after the playing one: the queue panel's
    /// "Next up". One model for the app's life, so the list keeps its place.
    up_next: Rc<VecModel<ItemData>>,
    extending: bool,
    /// Where to start a song restored from the last run, by its video id
    /// and seconds. Used the first time that song plays.
    resume_at: Option<(String, f32)>,
    shuffle: bool,
    /// The queue as it was before shuffle, to put back when shuffle ends.
    unshuffled: Option<Vec<Item>>,
    repeat: Repeat,
    /// Smart shuffle: shuffle that mixes in suggested songs.
    smart: bool,
    /// Video ids of the songs smart shuffle mixed in, to take out again.
    suggested: HashSet<String>,
}

/// The repeat button: off, the whole queue, or the one song.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
    pub fn number(self) -> i32 {
        self as i32
    }
    pub fn from_number(n: i64) -> Repeat {
        match n {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        }
    }
}

/// The song at queue position `index` first, the rest shuffled after it.
fn shuffled_from(mut items: Vec<Item>, index: usize) -> Vec<Item> {
    if index >= items.len() {
        return shuffled(items);
    }
    let first = items.remove(index);
    let mut out = vec![first];
    out.extend(shuffled(items));
    out
}

/// Puts the queue back in its order from before shuffle. Songs added
/// since go at the end. Returns the queue and where `current` is in it.
fn unshuffle(original: Vec<Item>, queue: &[Item], current: &Item) -> (Vec<Item>, usize) {
    let id = |i: &Item| i.video_id.clone();
    let mut out = original;
    for item in queue {
        if !out.iter().any(|o| id(o) == id(item)) {
            out.push(item.clone());
        }
    }
    out.retain(|o| queue.iter().any(|q| id(q) == id(o)));
    let index = out.iter().position(|o| id(o) == id(current)).unwrap_or(0);
    (out, index)
}

#[derive(Default)]
struct LyricsState {
    /// The player generation these lyrics belong to.
    generation: u64,
    synced: bool,
    /// Start, end, vertical offset and line number of every focusable row.
    rows: Vec<(i32, i32, f32, i32)>,
    /// The lyrics as found, to break into rows again when the space for
    /// them changes width.
    found: Option<lyrics::Lyrics>,
    /// The rows on show, so their blurred images can be added once drawn.
    model: Option<Rc<VecModel<LyricRow>>>,
    /// Counts the times the rows were laid out, so blurred images drawn
    /// for an older layout are dropped.
    layout: u64,
    /// The language the lyrics on show are translated into, or being
    /// translated into. Empty for none.
    translated_to: &'static str,
}

/// How much of the current track has really played. Drives scrobbling.
#[derive(Default)]
struct Listen {
    generation: u64,
    played: Duration,
    last: Option<Instant>,
    scrobbled: bool,
    /// Whether this play has counted as a listen in the history yet.
    counted: bool,
    was_playing: bool,
}

/// A song opened ahead so the playing one can crossfade into it.
struct FadeNext {
    /// Names it to the audio engine.
    token: u64,
    /// The player generation of the song it follows.
    generation: u64,
    /// Its queue position and video id, to notice the queue changing.
    index: usize,
    video_id: String,
    /// Its length as yt-dlp gave it, in seconds. Zero when unknown.
    duration: f32,
    /// With crossfade off: the engine was told to start it the moment
    /// the playing song ends.
    armed: bool,
}

/// The mix from one song into the next, and the settings it came from.
struct Planned {
    from: String,
    into: String,
    settings: transition::Settings,
    plan: mix::Transition,
}

/// Volume levelling choices in Settings: the loudness each aims for, in
/// LKFS, and how far it may lift a quiet song, in dB. Off is first.
const LEVELLING: [(f32, f32); 4] = [(0.0, 0.0), (-19.0, 0.0), (-14.0, 0.0), (-11.0, 4.0)];

/// The volume levelling choice saved in Settings. Normal when none is.
/// Whether closing the window keeps Tonearm playing in the tray.
fn load_close_to_tray() -> bool {
    store::load("close-to-tray").map_or(true, |text| text.trim() != "0")
}

fn load_levelling() -> i32 {
    store::load("levelling")
        .and_then(|text| text.trim().parse().ok())
        .filter(|choice: &i32| (0..LEVELLING.len() as i32).contains(choice))
        .unwrap_or(2)
}

/// Sleep timer choices, in minutes, after Off. The last is the end of
/// the playing song.
const SLEEP_MINUTES: [u64; 5] = [5, 15, 30, 45, 60];
const SLEEP_END_OF_SONG: i32 = 6;

/// The sleep timer.
#[derive(Clone, Copy, Default, PartialEq)]
enum Sleep {
    #[default]
    Off,
    /// Pauses at this time.
    At(Instant),
    /// Stops once the playing song ends.
    EndOfSong,
}

/// Smart shuffle mixes in one suggested song after every this many.
const SMART_EVERY: usize = 3;
/// And no more than this many at a time.
const SMART_MOST: usize = 12;

/// Crossfade lengths offered in Settings, in seconds. Zero is off.
const CROSSFADE_CHOICES: [u32; 9] = [0, 1, 2, 3, 4, 6, 8, 10, 12];
/// How long before the fade the next song starts opening. Its link and
/// first bytes can take a while to come.
const PREPARE_AHEAD: f32 = 30.0;
/// A fade shorter than this, left after a late start, is not worth it.
const SHORTEST_FADE: f32 = 0.5;
/// How long before a beat matched mix the player hands it to the engine,
/// which then starts it to the frame. Covers the slow tick and the audio
/// thread's position reports.
const MIX_LEAD: f32 = 0.5;
/// Opening the next song this far ahead leaves time to fetch and analyze
/// it before a beat matched mix, which can start well before the end.
const PREPARE_AHEAD_MIXING: f32 = 75.0;
/// How long mixes last in a playlist's mix mode when crossfade is off.
const MIX_MODE_SECONDS: f32 = 8.0;

/// The crossfade length saved in Settings. Off when none is saved.
fn load_crossfade() -> u32 {
    store::load("crossfade")
        .and_then(|text| text.trim().parse().ok())
        .filter(|seconds| CROSSFADE_CHOICES.contains(seconds))
        .unwrap_or(0)
}

/// Languages lyrics can be translated into in Settings, as the tag Google
/// Translate takes. The first, empty, is off. Names match the dropdown.
const TRANSLATE_CHOICES: [&str; 13] = [
    "", "en", "es", "fr", "de", "it", "pt", "ru", "ja", "ko", "zh-CN", "zh-TW", "id",
];

/// The language saved in Settings to translate lyrics into, as an index
/// into `TRANSLATE_CHOICES`. Off when none is saved.
fn load_translate() -> usize {
    store::load("translate")
        .and_then(|tag| TRANSLATE_CHOICES.iter().position(|&t| t == tag.trim()))
        .unwrap_or(0)
}

struct Cover {
    generation: u64,
    frames: Vec<motion::Frame>,
    index: usize,
    due: Instant,
}

/// A looping clip in place of the cover: a Spotify Canvas or Apple's
/// moving cover, and which views it shows in.
struct Motion {
    /// Unique to this clip, so frames from one stopped since are dropped.
    id: u64,
    clip: canvas::Clip,
    /// Shows in the Now playing panel.
    panel: bool,
    /// Shows in expanded Now playing.
    wide: bool,
}

/// What the Now playing panel shows for the cover, as chosen in Settings.
const PANEL_CANVAS: i32 = 0;
const PANEL_ANIMATED: i32 = 1;
/// What expanded Now playing shows for the cover.
const WIDE_ANIMATED: i32 = 0;

/// The cover choices in Settings, for the panel and the expanded view.
/// The moving album cover in both when none are saved.
fn load_cover_choices() -> (i32, i32) {
    let text = store::load("cover-choices").unwrap_or_default();
    let mut parts = text.trim().split(',').map(|p| p.parse::<i32>().ok());
    let panel = parts.next().flatten().filter(|c| (0..3).contains(c)).unwrap_or(PANEL_ANIMATED);
    let wide = parts.next().flatten().filter(|c| (0..2).contains(c)).unwrap_or(WIDE_ANIMATED);
    (panel, wide)
}

/// The song going out during a crossfade. Its moving cover or Canvas keeps
/// playing while it fades out under the new song's.
struct Outgoing {
    cover: Option<Cover>,
    canvas: Option<Motion>,
    /// The song going out, for the pulse on its row's transition line.
    video_id: String,
    started: Instant,
    until: Instant,
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
    taste: RefCell<history::History>,
    profile: RefCell<Profile>,
    /// The link to the signed in account's picture.
    account_photo: RefCell<String>,
    /// One thread, so history saves land in the order they were made.
    disk: pool::Pool,
    /// When the playback session was last saved while a song played.
    session_saved: Cell<Instant>,
    /// Gets stream links ahead of a click. Its own threads, as a link can
    /// take many seconds and must not hold up pages.
    warm: pool::Pool,
    /// The last song a failed stream was retried for, so it retries once.
    retried: RefCell<Option<String>>,
    /// Songs whose links are being got ahead right now.
    warming: Rc<RefCell<HashSet<String>>>,
    /// The album, playlist, mix or artist the queue was started from, by
    /// its key. Empty for a song's radio and the like. Lights up that
    /// item's play button while it plays.
    playing_from: RefCell<String>,
    player: RefCell<Player>,
    lyrics: RefCell<LyricsState>,
    cover: RefCell<Option<Cover>>,
    /// The clip looping as the cover: in the panel, or in both views when
    /// they show the same one.
    canvas: RefCell<Option<Motion>>,
    /// The clip in expanded Now playing, when it is not the panel's.
    wide: RefCell<Option<Motion>>,
    /// Counts clips, for `Motion::id`.
    clips: Cell<u64>,
    /// The cover choices in Settings: the panel's and the expanded view's.
    cover_choices: Cell<(i32, i32)>,
    outgoing: RefCell<Option<Outgoing>>,
    clock: Cell<(u64, Instant)>,
    /// Seconds a song crossfades into the next one over. Zero is off.
    crossfade: Cell<u32>,
    /// Songs' analyses read from the library, by video id.
    analyses: RefCell<HashMap<String, Rc<analysis::Analysis>>>,
    /// Songs looked for in the library and not found, so the next tick does
    /// not look again. Cleared for a song when its analysis comes in.
    unanalyzed: RefCell<HashSet<String>>,
    /// The mix into the next song, once worked out.
    planned: RefCell<Option<Planned>>,
    /// Playlists with mix mode on, by page key.
    mixed_playlists: RefCell<HashSet<String>>,
    /// The transition open in the editor.
    editing: RefCell<Option<mixing::Editing>>,
    /// Unsaved choices from the editor, played in place of the saved ones
    /// between these two songs, for a preview.
    previewing: RefCell<Option<(String, String, transition::Settings)>>,
    /// The preview playing, to move the editor's playhead.
    preview_track: RefCell<Option<mixing::PreviewTrack>>,
    /// The playhead's last place in seconds of the transition, and when.
    /// It runs on this clock and leans towards the reports, which come in
    /// steps.
    playhead_clock: Cell<Option<(f64, Instant)>>,
    /// When the preview's spinner started turning.
    spinner_from: Cell<Option<Instant>>,
/// The language lyrics are translated into: an index into
    /// `TRANSLATE_CHOICES`, zero for off.
    translate: Cell<usize>,
    /// The next song, being opened ahead for a crossfade.
    fade_next: RefCell<Option<FadeNext>>,
    /// Counts the songs opened ahead, so each gets its own token.
    fade_token: Cell<u64>,
    /// Volume levelling: an index into `LEVELLING`.
    levelling: Cell<i32>,
    /// Each song's loudness in LKFS, by video id, as YouTube measured it.
    loudness: RefCell<HashMap<String, f32>>,
    /// Video ids whose loudness is being looked up.
    loudness_asked: RefCell<HashSet<String>>,
    sleep: Cell<Sleep>,
    timer: Timer,
    fast: Cell<bool>,
    /// Moves the playing bars while a song plays and the window is seen.
    beat_timer: Timer,
    /// Waves the expanded view's background while it is open.
    wave_timer: Timer,
    /// The still cover's blur that the background waves from.
    backdrop_base: RefCell<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>>,
    /// Whether closing the window sends Tonearm to the tray.
    close_to_tray: Cell<bool>,
    /// Whether the window is closed to the tray.
    hidden: Cell<bool>,
    /// Whether the taskbar buttons and tray are hooked up.
    attached: Cell<bool>,
    /// Whether the window was minimised at the last tick.
    minimized: Cell<bool>,
    services: services::Services,
    settings: RefCell<services::Settings>,
    lastfm_token: RefCell<String>,
    listen: RefCell<Listen>,
    toast_timer: Timer,
    /// What the toast's button does, while it shows one.
    toast_action: RefCell<Option<Box<dyn Fn(&Rc<App>)>>>,
    release: RefCell<Option<update::Release>>,
    suggest: RefCell<Suggest>,
    suggest_timer: Timer,
    /// What was searched for lately, newest first.
    recent_searches: RefCell<Vec<String>>,
    /// What each line of the open right click menu does.
    menu: RefCell<Vec<menu::Action>>,
    /// What the name prompt open now is for.
    prompt: RefCell<Option<menu::Prompt>>,
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

/// The average colour of a picture, scaled so its brightest channel is
/// `peak`. Dark enough that white text on top stays readable.
fn tint_of(image: &Image, peak: f32) -> Option<slint::Color> {
    let pixels = image.to_rgba8()?;
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    for pixel in pixels.as_slice().iter().step_by(7) {
        // Faded corners would pull the colour toward black.
        if pixel.a < 128 {
            continue;
        }
        r += pixel.r as u32;
        g += pixel.g as u32;
        b += pixel.b as u32;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let (r, g, b) = ((r / n) as f32, (g / n) as f32, (b / n) as f32);
    let scale = peak / r.max(g).max(b).max(1.0);
    let channel = |v: f32| (v * scale).clamp(0.0, 255.0) as u8;
    Some(slint::Color::from_rgb_u8(channel(r), channel(g), channel(b)))
}

fn clock_text(seconds: f32) -> SharedString {
    let total = seconds.max(0.0) as u32;
    format!("{}:{:02}", total / 60, total % 60).into()
}

/// Search results put the kind first and the length last in the subtitle:
/// "Song • Daft Punk • Discovery • 5:21". Returns the artist and the length.
fn canvas_keys(s: &services::Settings) -> canvas::Keys {
    canvas::Keys {
        cookie: s.spotify_cookie.clone(),
        client_id: s.spotify_id.clone(),
        client_secret: s.spotify_secret.clone(),
    }
}

/// Where the song at `index` sits after the song at `from` moves to `to`.
fn index_after_move(index: usize, from: usize, to: usize) -> usize {
    if from == index {
        to
    } else if from < index && to >= index {
        index - 1
    } else if from > index && to <= index {
        index + 1
    } else {
        index
    }
}

fn credits(item: &Item) -> (String, f32) {
    let (artist, _, seconds) = credits_with_album(item);
    (artist, seconds)
}

/// The first credited artist, for scrobbling. YouTube's links name each
/// artist on its own, so a duo like "Simon & Garfunkel" stays whole. With
/// no links, the credit line is cut at the first ", " or " & ".
fn lead_artist(item: &Item, artist: &str) -> String {
    if let Some((name, _)) = item.artists.iter().find(|(name, _)| !name.trim().is_empty()) {
        return name.trim().to_string();
    }
    let cut = [", ", " & "]
        .iter()
        .filter_map(|sep| artist.find(sep))
        .min()
        .unwrap_or(artist.len());
    artist[..cut].trim().to_string()
}

/// As `credits`, with the album in the middle. The album is empty when the
/// subtitle has none, as for videos, which show a view count there.
fn credits_with_album(item: &Item) -> (String, String, f32) {
    let parts: Vec<&str> = item.subtitle.split(" • ").map(str::trim).collect();
    let labels = ["Song", "Video", "Single", "Album", "EP", "Episode"];
    let is_count = |p: &str| [" views", " plays", " likes"].iter().any(|w| p.ends_with(w));
    // "2:38" is the length, not a name.
    let is_duration =
        |p: &str| p.contains(':') && p.chars().all(|c| c.is_ascii_digit() || c == ':');
    let mut names = parts
        .iter()
        .filter(|p| !labels.contains(*p) && !is_duration(p))
        .copied();
    // A play count in the artist's place is no artist. Fall back to the
    // linked artists, which the subtitle does not always carry.
    let artist = match names.next() {
        Some(p) if !is_count(p) => p.to_string(),
        _ => item
            .artists
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    };
    let album = names
        .next()
        .filter(|p| {
            let count = [" views", " plays", " likes"].iter().any(|w| p.ends_with(w));
            let year = p.len() == 4 && p.chars().all(|c| c.is_ascii_digit());
            !count && !year && !p.contains(':')
        })
        .unwrap_or("")
        .to_string();
    let mut seconds = item.duration_secs();
    if seconds == 0.0 {
        if let Some(last) = parts.iter().rev().find(|p| is_duration(p)) {
            seconds = Item {
                duration: last.to_string(),
                ..Item::default()
            }
            .duration_secs();
        }
    }
    (artist, album, seconds)
}

/// Splits a suggested search into the part that matches what was typed,
/// shown in bold, and the rest. When it does not start with the typed
/// text, nothing is bold.
fn split_typed(suggestion: &str, typed: &str) -> (String, String) {
    let mut end = 0;
    let mut typed_chars = typed.chars();
    for (index, c) in suggestion.char_indices() {
        match typed_chars.next() {
            Some(t) if t.to_lowercase().eq(c.to_lowercase()) => end = index + c.len_utf8(),
            Some(_) => return (String::new(), suggestion.to_string()),
            None => break,
        }
    }
    if typed_chars.next().is_some() {
        return (String::new(), suggestion.to_string());
    }
    (suggestion[..end].to_string(), suggestion[end..].to_string())
}

/// "Artist", "Song • Journey", "Album • Daft Punk": what an item is and
/// who made it, without the play counts YouTube Music adds.
fn suggestion_subtitle(item: &Item) -> String {
    if item.kind == Kind::Artist {
        return "Artist".into();
    }
    let labels = [
        "Song", "Video", "Single", "Album", "EP", "Playlist", "Episode", "Podcast", "Profile",
    ];
    let counts = [" views", " plays", " likes", " audience", " subscribers", " songs"];
    let parts: Vec<&str> = item
        .subtitle
        .split(" • ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let label = parts
        .first()
        .filter(|p| labels.contains(p))
        .copied()
        .unwrap_or(match item.kind {
            Kind::Song => "Song",
            Kind::Album => "Album",
            Kind::Playlist => "Playlist",
            _ => "",
        });
    let by = parts
        .iter()
        .find(|p| !labels.contains(p) && !counts.iter().any(|c| p.ends_with(c)));
    match by {
        Some(by) if !label.is_empty() => format!("{label} • {by}"),
        Some(by) => by.to_string(),
        None => label.to_string(),
    }
}

impl App {
    fn toast(self: &Rc<Self>, message: impl Into<SharedString>) {
        self.show_toast(message.into(), "", None, Duration::from_secs(5));
    }

    /// A toast with a button. It stays longer, so there is time to click.
    fn toast_with(
        self: &Rc<Self>,
        message: impl Into<SharedString>,
        label: &str,
        action: impl Fn(&Rc<App>) + 'static,
    ) {
        self.show_toast(message.into(), label, Some(Box::new(action)), Duration::from_secs(12));
    }

    fn show_toast(
        self: &Rc<Self>,
        message: SharedString,
        label: &str,
        action: Option<Box<dyn Fn(&Rc<App>)>>,
        lasts: Duration,
    ) {
        self.ui.set_toast(message);
        self.ui.set_toast_action(label.into());
        *self.toast_action.borrow_mut() = action;
        let weak = Rc::downgrade(self);
        self.toast_timer.start(TimerMode::SingleShot, lasts, move || {
            if let Some(app) = weak.upgrade() {
                app.hide_toast();
            }
        });
    }

    fn hide_toast(&self) {
        self.ui.set_toast("".into());
        self.ui.set_toast_action("".into());
        self.toast_action.borrow_mut().take();
    }

    fn toast_clicked(self: &Rc<Self>) {
        let action = self.toast_action.borrow_mut().take();
        self.toast_timer.stop();
        self.hide_toast();
        if let Some(action) = action {
            action(self);
        }
    }

    // ---------------------------------------------------------- thumbnails

    /// Screen pixels for `logical` pixels of the UI. Art is fetched at this
    /// size so it stays sharp on a scaled display.
    fn physical(&self, logical: u32) -> u32 {
        (logical as f32 * self.ui.window().scale_factor()).round() as u32
    }

    /// A square thumbnail from the cache, or None while it loads. `px` is in
    /// logical pixels; zero means the row draws no art, so nothing is fetched.
    fn thumb(self: &Rc<Self>, url: &str, px: u32, target: Target) -> Option<Image> {
        if url.is_empty() || px == 0 {
            return None;
        }
        let px = self.physical(px);
        let key = images::Cache::key(url, px);
        self.load_art(key, url, target, move |http, url| images::fetch(http, url, px))
    }

    /// A wide picture, such as an artist banner, `width` pixels across.
    fn wide_thumb(self: &Rc<Self>, url: &str, width: u32, target: Target) -> Option<Image> {
        if url.is_empty() {
            return None;
        }
        // Banners are wide already, so a big display gets no more than this.
        let width = self.physical(width).min(2048);
        let key = images::wide_key(url, width);
        self.load_art(key, url, target, move |http, url| {
            images::fetch_wide(http, url, width)
        })
    }

    fn load_art(
        self: &Rc<Self>,
        key: String,
        url: &str,
        target: Target,
        fetch: impl FnOnce(&ureq::Agent, &str) -> Option<images::Pixels> + Send + 'static,
    ) -> Option<Image> {
        if let Some(pixels) = self.cache.borrow_mut().get(&key) {
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
            let pixels = fetch(&http, &url);
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
                    Some(Place::Row(row) | Place::Extra(row)) => {
                        if let Some(mut data) = page.rows.row_data(*row) {
                            data.item.art = image;
                            data.item.has_art = true;
                            page.rows.set_row_data(*row, data);
                        }
                    }
                    Some(Place::Card(model, index)) => set(model, *index),
                    Some(Place::Gone) | None => {}
                }
            }
            Target::Queue { generation, index } => {
                let player = self.player.borrow();
                if player.queue_generation == generation {
                    set(&player.model, index);
                    if index > player.index {
                        set(&player.up_next, index - player.index - 1);
                    }
                }
            }
            Target::Suggest { generation, row } => {
                let suggest = self.suggest.borrow();
                if suggest.generation != generation {
                    return;
                }
                if let Some(mut data) = suggest.rows.row_data(row) {
                    data.item.art = image;
                    data.item.has_art = true;
                    suggest.rows.set_row_data(row, data);
                }
            }
            Target::Track { generation } => {
                // The still cover always goes in. Moving covers and the
                // Canvas play in a layer of their own on top of it.
                if self.player.borrow().generation == generation {
                    self.set_tint(&image);
                    if !self.ui.get_track_motion_on() {
                        self.backdrop_of(&image);
                    }
                    self.ui.set_track_art(image);
                    self.ui.set_track_has_art(true);
                }
            }
            Target::Hero { generation } => {
                if self.page.borrow().generation == generation {
                    self.set_hero_art(image);
                }
            }
            Target::Account => {
                if self.ui.get_signed_in() {
                    self.ui.set_account_art(image);
                    self.ui.set_account_has_art(true);
                }
            }
            Target::ProfilePhoto => {
                if self.ui.get_signed_in() {
                    self.ui.set_profile_art(image);
                    self.ui.set_profile_has_art(true);
                }
            }
            Target::Profile { generation, index } => {
                let profile = self.profile.borrow();
                if profile.generation == generation {
                    if let Some(model) = &profile.model {
                        set(model, index);
                    }
                }
            }
        }
    }

    /// Tints the player and side panel with the cover's average colour,
    /// pulled toward a mid brightness so text stays readable on top.
    fn set_tint(&self, image: &Image) {
        if let Some(colour) = tint_of(image, 150.0) {
            self.ui.set_tint(colour);
        }
    }

    fn item_data(self: &Rc<Self>, item: &Item, id: usize, px: u32, target: Target) -> ItemData {
        let art = self.thumb(&item.thumb, px, target);
        let (runs, short) = detail::link_runs(&item.subtitle, item);
        let (album, album_browse) = item.album.clone().unwrap_or_default();
        ItemData {
            id: id as i32,
            key: history::key_of(item).unwrap_or_default().into(),
            title: item.title.as_str().into(),
            subtitle: item.subtitle.as_str().into(),
            runs: ModelRc::new(VecModel::from(runs)),
            short_runs: ModelRc::new(VecModel::from(short)),
            album: album.into(),
            album_browse: album_browse.into(),
            duration: item.duration.as_str().into(),
            has_art: art.is_some(),
            art: art.unwrap_or_default(),
            round: item.kind == Kind::Artist,
            downloaded: item
                .video_id
                .as_deref()
                .is_some_and(offline::is_downloaded),
            downloading: item.video_id.as_deref().is_some_and(|id| {
                !offline::is_downloaded(id) && offline::is_downloading(id)
            }),
            progress: item.video_id.as_deref().map(offline::progress).unwrap_or(0.0),
            explicit: item.explicit,
            trend: match item.trend {
                Trend::None => 0,
                Trend::Up => 1,
                Trend::Down => 2,
                Trend::Same => 3,
            },
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
                // Downloads queue the page, so they keep playing offline.
                radio_on_click: !matches!(route, Route::Browse { .. } | Route::Downloads),
                query: match &route {
                    Route::Search(query, _) => Some(query.clone()),
                    _ => None,
                },
                ..PageState::default()
            };
            self.ui.set_rows(ModelRc::from(page.rows.clone()));
            self.ui.set_sort_column(0);
            self.ui.set_sort_descending(false);
            page.generation
        };
        // A search keeps its title while it loads, so the tabs under it
        // stay put when another one is picked.
        let title = match &route {
            Route::Search(query, _) => format!("Results for \u{201c}{query}\u{201d}"),
            _ => String::new(),
        };
        self.ui.set_page_title(title.into());
        self.ui.set_page_subtitle("".into());
        self.ui.set_page_playable(false);
        // The kind guessed from the link picks the skeleton until the page
        // arrives. The last page's colour must not flash on this one.
        let guess = detail::guess_kind(&route);
        self.ui.set_header(PageHeader {
            kind: guess,
            banner: guess == 1,
            round: guess == 1,
            ..PageHeader::default()
        });
        self.ui.set_page_tint(slint::Color::from_rgb_u8(0x2b, 0x24, 0x40));
        self.ui.set_loading(true);
        self.ui.set_can_back(self.history.borrow().len() > 1);
        self.show_now();
        self.ui.set_nav(match route {
            Route::Home => 0,
            Route::Explore => 1,
            Route::Library => 2,
            Route::Downloads => 4,
            _ => -1,
        });
        self.ui.set_search_tab(match &route {
            Route::Search(_, tab) => *tab as i32,
            _ => -1,
        });
        if let Route::Downloads = route {
            let mut page = offline::downloads_page();
            page.header = Some(detail::downloads_header(&page));
            self.show(generation, Ok(page));
            return;
        }
        // The search list already fetched these results while typing.
        if let Route::Search(query, 0) = &route {
            let cached = self.suggest.borrow().cache.get(&query.to_lowercase()).cloned();
            if let Some(found) = cached.filter(|c| c.at.elapsed() < SUGGEST_FRESH) {
                if !found.page.shelves.is_empty() {
                    self.show(generation, Ok(found.page));
                    return;
                }
            }
        }
        let client = self.client.clone();
        // Home is put together from several pages and the listening history.
        if let Route::Home = route {
            self.api.run(move || {
                let parts = home::fetch(&client);
                on_ui(move |app| {
                    let page = home::compose(parts, &app.taste.borrow());
                    app.show(generation, page);
                });
            });
            return;
        }
        self.api.run(move || {
            let result = match &route {
                Route::Home => client.browse("FEmusic_home", None),
                Route::Explore => explore(&client),
                Route::Library => client.browse("FEmusic_library_landing", None),
                Route::Browse { id, params } => client.browse(id, params.as_deref()),
                Route::Search(query, tab) => client.search_in(query, *tab),
                Route::Downloads => Ok(offline::downloads_page()),
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
            Ok(mut page) => {
                let (query, first) = {
                    let state = self.page.borrow();
                    (state.query.clone(), state.items.is_empty())
                };
                if let Some(query) = query {
                    self.tailor_results(&query, &mut page, first);
                }
                // Only the first page of a detail page has the header.
                if first {
                    match page.header.take() {
                        Some(header) => self.show_header(generation, header),
                        None => self.ui.set_header(PageHeader::default()),
                    }
                }
                let detail = self.page.borrow().header.is_some();
                if !page.title.is_empty() && !detail {
                    self.ui.set_page_title(page.title.as_str().into());
                    self.ui.set_page_subtitle(page.subtitle.as_str().into());
                }
                if page.shelves.is_empty() && first && !detail {
                    self.toast("Nothing here yet");
                }
                self.append(page);
            }
            Err(message) => {
                // An empty page keeps no hero, or the skeleton would stay.
                if self.page.borrow().items.is_empty() {
                    self.ui.set_header(PageHeader::default());
                }
                self.toast(message)
            }
        }
    }

    /// Puts a detail page's hero on screen and starts loading its art.
    fn show_header(self: &Rc<Self>, generation: u64, header: Header) {
        let mut data = detail::header_data(&header, self.client.signed_in());
        if header.kind == PageKind::Artist && !header.banner {
            data.round = true;
        }
        self.show_mix_header(&mut data, header.kind == PageKind::Playlist);
        let art = if header.banner {
            self.wide_thumb(&header.thumb, 1280, Target::Hero { generation })
        } else {
            self.thumb(&header.thumb, 440, Target::Hero { generation })
        };
        self.ui.set_header(data);
        self.page.borrow_mut().header = Some(header);
        if let Some(image) = art {
            self.set_hero_art(image);
        }
    }

    fn set_hero_art(&self, image: Image) {
        let mut data = self.ui.get_header();
        if let Some(colour) = tint_of(&image, 120.0) {
            self.ui.set_page_tint(colour);
        }
        data.art = image;
        data.has_art = true;
        self.ui.set_header(data);
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
        let (kind, byline, mut numbered, mut mores) = {
            let page = self.page.borrow();
            let header = page.header.as_ref();
            (
                header.map(|h| h.kind),
                header.map(|h| h.byline.clone()).unwrap_or_default(),
                page.numbered,
                page.mores.len(),
            )
        };
        let first = rows.row_count() == 0;
        // A chart's numbers carry up and down marks, in a wider column.
        if first {
            let chart = new.shelves.iter().flat_map(|s| &s.items).any(|i| i.trend != Trend::None);
            self.ui.set_chart(chart);
        }
        // How the track list of this kind of page looks: 1 numbered with
        // no art for albums, 2 numbered with art for playlists.
        let track_style = match kind {
            Some(PageKind::Album | PageKind::Single) => 1,
            Some(PageKind::Playlist | PageKind::Other) => 2,
            _ => 0,
        };
        let mut new_mores = Vec::new();
        if first && kind.is_some() {
            rows.push(RowData {
                kind: 4,
                ..RowData::default()
            });
            // A single has a track or two, which need no column titles.
            let listed = new.shelves.first().is_some_and(|s| !s.carousel);
            if listed && matches!(kind, Some(PageKind::Album | PageKind::Playlist)) {
                rows.push(RowData {
                    kind: 6,
                    style: track_style,
                    ..RowData::default()
                });
            }
        }
        let about = new
            .continuation
            .is_none()
            .then(|| self.page.borrow().header.as_ref().map(|h| h.about.clone()))
            .flatten()
            .filter(|a| first && !a.is_empty());
        for shelf in new.shelves {
            if !shelf.title.is_empty() {
                rows.push(RowData {
                    kind: 0,
                    title: shelf.title.as_str().into(),
                    more: shelf.more.is_some(),
                    more_id: mores as i32,
                    ..RowData::default()
                });
                if let Some(more) = shelf.more.clone() {
                    new_mores.push(more);
                    mores += 1;
                }
            }
            // The artist's top songs are numbered like a chart.
            let style = match kind {
                Some(PageKind::Artist) if shelf.title == "Popular" => 2,
                Some(PageKind::Artist) => 0,
                // Explore's chart, listed by `chart_top_songs`.
                None if shelf.title == CHART_TITLE && !shelf.carousel => 2,
                _ => track_style,
            };
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
                for (index, item) in shelf.items.into_iter().enumerate() {
                    // YouTube Music's pick gets a card of its own, with big art.
                    let top = shelf.title == "Top result" && index == 0;
                    // Numbered album rows draw no art, so fetch none.
                    let px = if top { 128 } else if style == 1 { 0 } else { 64 };
                    let mut data = self.item_data(
                        &item,
                        next_id,
                        px,
                        Target::Page {
                            generation,
                            id: next_id,
                        },
                    );
                    // On an album every track is by the album's artist.
                    // Saying so on each row is noise, so only other artists show.
                    if style == 1 && detail::only_by(&item, &byline) {
                        data.subtitle = "".into();
                    }
                    let number = match style {
                        0 => 0,
                        _ if kind == Some(PageKind::Artist) => index + 1,
                        _ => {
                            numbered += 1;
                            numbered
                        }
                    };
                    places.push(Place::Row(rows.row_count()));
                    rows.push(RowData {
                        kind: if top { 7 } else { 1 },
                        item: data,
                        style,
                        number: number as i32,
                        ..RowData::default()
                    });
                    items.push(item);
                    next_id += 1;
                }
            }
        }
        if let Some(about) = about {
            rows.push(RowData {
                kind: 0,
                title: "About".into(),
                ..RowData::default()
            });
            rows.push(RowData {
                kind: 5,
                title: about.into(),
                ..RowData::default()
            });
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
        page.numbered = numbered;
        page.mores.extend(new_mores);
        let playable = !page.radio_on_click
            && page
                .items
                .iter()
                .zip(&page.places)
                .any(|(i, p)| i.video_id.is_some() && matches!(p, Place::Row(_)));
        // An artist page plays its top songs, or its shuffle when it has none.
        let artist_shuffle = page.header.as_ref().is_some_and(|h| h.shuffle.is_some());
        let artist = kind == Some(PageKind::Artist);
        let sorted = page.sort != (0, false);
        drop(page);
        if sorted {
            // A sorted list is only right once all of it is here.
            self.apply_sort();
            self.load_more();
        }
        self.show_mix_rows();
        self.offer_recommended();
        self.ui.set_page_playable(playable);
        if kind.is_some() {
            let mut data = self.ui.get_header();
            data.can_play = playable || (artist && artist_shuffle);
            // Episodes of a podcast make no sense shuffled.
            let podcast = kind == Some(PageKind::Podcast);
            data.can_shuffle = (playable && !podcast) || artist_shuffle;
            self.ui.set_header(data);
            self.show_download_header();
        }
    }

    fn load_more(self: &Rc<Self>) {
        let (generation, token, search) = {
            let mut page = self.page.borrow_mut();
            if page.loading_more {
                return;
            }
            let Some(token) = page.continuation.take() else {
                return;
            };
            page.loading_more = true;
            (page.generation, token, page.query.is_some())
        };
        let client = self.client.clone();
        self.api.run(move || {
            let result = if search {
                client.search_more(&token)
            } else {
                client.more(&token)
            };
            on_ui(move |app| app.show(generation, result));
        });
    }

    fn back(self: &Rc<Self>) {
        if matches!(self.ui.get_nav(), 3 | 5) {
            // Settings and the profile sit on top of the page. Leaving them
            // shows the page again.
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

    /// A click on a track list's column title. A first click sorts by
    /// the column, a second turns it around, and a third goes back to the
    /// list's own order. "#" just turns the list's own order around.
    fn sort_by(self: &Rc<Self>, column: i32) {
        let (current, descending) = self.page.borrow().sort;
        let next = match (column, current == column, descending) {
            (0, true, false) => (0, true),
            (0, ..) | (_, true, true) => (0, false),
            (_, true, false) => (column, true),
            (_, false, _) => (column, false),
        };
        self.page.borrow_mut().sort = next;
        self.ui.set_sort_column(next.0);
        self.ui.set_sort_descending(next.1);
        self.apply_sort();
        self.show_mix_rows();
        if next != (0, false) {
            self.load_more();
        }
    }

    /// Puts the songs under the column titles in the order picked.
    fn apply_sort(&self) {
        let mut page = self.page.borrow_mut();
        let rows = page.rows.clone();
        let count = rows.row_count();
        let Some(start) = (0..count).position(|i| rows.row_data(i).is_some_and(|r| r.kind == 6))
        else {
            return;
        };
        let start = start + 1;
        let mut end = start;
        while end < count && rows.row_data(end).is_some_and(|r| r.kind == 1) {
            end += 1;
        }
        // Which item each row shows.
        let mut ids = vec![None; end - start];
        for (id, place) in page.places.iter().enumerate() {
            if let Place::Row(row) = place {
                if (start..end).contains(row) {
                    ids[row - start] = Some(id);
                }
            }
        }
        let (column, descending) = page.sort;
        let mut list = Vec::with_capacity(ids.len());
        for (offset, id) in ids.into_iter().enumerate() {
            let (Some(id), Some(data)) = (id, rows.row_data(start + offset)) else {
                return;
            };
            let key = match column {
                1 => data.item.title.to_lowercase(),
                2 => data.item.album.to_lowercase(),
                3 => mixing::bpm_sort_key(&data.bpm),
                4 => mixing::key_sort_key(&data.camelot),
                _ => String::new(),
            };
            list.push((id, key, data));
        }
        list.sort_by(|a, b| {
            let order = if column == 0 { a.0.cmp(&b.0) } else { a.1.cmp(&b.1) };
            let order = if descending { order.reverse() } else { order };
            // Songs with no album (or title) go last either way.
            let blank = if column == 0 {
                std::cmp::Ordering::Equal
            } else {
                a.1.is_empty().cmp(&b.1.is_empty())
            };
            blank.then(order).then(a.0.cmp(&b.0))
        });
        for (offset, (id, _, data)) in list.into_iter().enumerate() {
            let row = start + offset;
            if !matches!(page.places[id], Place::Row(r) if r == row) {
                page.places[id] = Place::Row(row);
                rows.set_row_data(row, data);
            }
        }
    }

    /// Songs on the current page that sit in a vertical list, top to bottom.
    fn page_songs(&self) -> Vec<(usize, Item)> {
        let page = self.page.borrow();
        let mut songs: Vec<_> = page
            .items
            .iter()
            .zip(&page.places)
            .enumerate()
            .filter_map(|(id, (item, place))| match place {
                Place::Row(row) if item.video_id.is_some() => Some((*row, id, item.clone())),
                _ => None,
            })
            .collect();
        songs.sort_by_key(|(row, ..)| *row);
        songs.into_iter().map(|(_, id, item)| (id, item)).collect()
    }

    fn activate(self: &Rc<Self>, id: usize) {
        let (item, radio) = {
            let page = self.page.borrow();
            let Some(item) = page.items.get(id) else {
                return;
            };
            (
                item.clone(),
                page.radio_on_click || matches!(page.places[id], Place::Card(..) | Place::Extra(_)),
            )
        };
        if item.video_id.is_some() && !radio {
            let songs = self.page_songs();
            let index = songs.iter().position(|(i, _)| *i == id).unwrap_or(0);
            let from = self.page_key();
            self.set_queue_from(songs.into_iter().map(|(_, item)| item).collect(), index, from);
        } else {
            self.open_item(item);
        }
    }

    /// Plays a song as the start of its radio, or opens an album, artist or
    /// playlist page, or plays a mix.
    fn open_item(self: &Rc<Self>, item: Item) {
        if item.video_id.is_none() {
            // Opening an artist, album or playlist says something about
            // taste. Songs are counted when they play.
            self.remember(&item, 0.5);
        }
        if innertube::is_video(&item) {
            // Songs over videos: play the album track the video is of, so
            // its radio is songs too. The video plays when there is none.
            let client = self.client.clone();
            self.api.run(move || {
                let song = client.song_for(&item).unwrap_or(item);
                on_ui(move |app| app.set_queue(vec![song], 0));
            });
        } else if item.video_id.is_some() {
            self.set_queue(vec![item], 0);
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

    /// Adds to the listening history and saves it in the background.
    fn remember(&self, item: &Item, weight: f64) {
        let text = {
            let mut history = self.taste.borrow_mut();
            history.record(item, weight);
            history.serialize()
        };
        self.disk.run(move || history::History::save(text));
    }

    fn clear_history(self: &Rc<Self>) {
        self.taste.borrow_mut().clear();
        self.disk.run(history::History::delete_file);
        self.recent_searches.borrow_mut().clear();
        self.disk.run(|| {
            let _ = store::save(RECENT_SEARCHES, "");
        });
        self.show_recent_searches();
        self.toast("Listening history cleared");
    }

    /// Tailors a page of search results: lists put what you play first,
    /// and a strong match from your history becomes the top result.
    fn tailor_results(&self, query: &str, page: &mut Page, first: bool) {
        let history = self.taste.borrow();
        if history.is_empty() {
            return;
        }
        for shelf in &mut page.shelves {
            // The top result's first item is YouTube Music's pick. Keep it
            // there and only reorder what follows it.
            let skip = usize::from(shelf.title == "Top result");
            if shelf.items.len() > skip + 1 {
                let mut rest = shelf.items.split_off(skip);
                history::rerank(&history, &mut rest);
                shelf.items.extend(rest);
            }
        }
        if !first {
            return;
        }
        // A title match on something you play often, roughly: the title
        // starts with what you typed and you have listened a few times.
        let Some((best, rank)) = history.matches(query, 1).into_iter().next() else {
            return;
        };
        if rank < 4.5 {
            return;
        }
        let key = history::key_of(&best);
        match page.shelves.iter_mut().find(|s| s.title == "Top result") {
            Some(top) => {
                if top.items.first().and_then(history::key_of) != key {
                    top.items.retain(|i| history::key_of(i) != key);
                    top.items.insert(0, best);
                }
            }
            None => page.shelves.insert(
                0,
                innertube::model::Shelf {
                    title: "Top result".into(),
                    carousel: false,
                    items: vec![best],
                    more: None,
                    ..Default::default()
                },
            ),
        }
    }

    // --------------------------------------------------------- suggestions

    /// The search text changed. Shows cached suggestions at once, or
    /// skeleton rows until a short pause in typing lets us ask for them.
    fn suggest_edited(self: &Rc<Self>, text: &str) {
        let query = text.trim().to_string();
        let key = query.to_lowercase();
        let (generation, cached) = {
            let mut suggest = self.suggest.borrow_mut();
            suggest.generation += 1;
            (suggest.generation, suggest.cache.get(&key).cloned())
        };
        self.suggest_timer.stop();
        if query.is_empty() {
            self.show_recent_searches();
            return;
        }
        if let Some(cached) = cached.filter(|c| c.at.elapsed() < SUGGEST_FRESH) {
            self.show_suggestions(generation, &query, cached.queries, cached.page);
            return;
        }
        {
            let mut suggest = self.suggest.borrow_mut();
            suggest.picks.clear();
            suggest.rows.set_vec(Vec::new());
        }
        self.ui.set_suggest_loading(true);
        let weak = Rc::downgrade(self);
        self.suggest_timer
            .start(TimerMode::SingleShot, SUGGEST_DELAY, move || {
                let Some(app) = weak.upgrade() else { return };
                let (client, query) = (app.client.clone(), query.clone());
                app.api.run(move || {
                    // The suggested searches and the real results, asked
                    // for side by side. The suggestion call's own items
                    // are YouTube Music's guesses and often differ from
                    // what the search returns, so they are not used.
                    let (queries, page) = std::thread::scope(|scope| {
                        let queries = scope.spawn(|| client.suggest(&query));
                        let page = client.search(&query);
                        let queries = queries.join().unwrap_or_else(|_| Err(String::new()));
                        (queries, page)
                    });
                    let result = match (queries, page) {
                        (Err(e), Err(_)) => Err(e),
                        (queries, page) => Ok(Suggested {
                            queries: queries.map(|(q, _)| q).unwrap_or_default(),
                            page: page.unwrap_or_default(),
                            at: Instant::now(),
                        }),
                    };
                    on_ui(move |app| app.got_suggestions(generation, &query, result));
                });
            });
    }

    fn got_suggestions(
        self: &Rc<Self>,
        generation: u64,
        query: &str,
        result: Result<Suggested, String>,
    ) {
        let current = self.suggest.borrow().generation == generation;
        match result {
            Ok(found) => {
                {
                    let mut suggest = self.suggest.borrow_mut();
                    if suggest.cache.len() > 300 {
                        suggest.cache.clear();
                    }
                    suggest.cache.insert(query.to_lowercase(), found.clone());
                }
                if current {
                    self.show_suggestions(generation, query, found.queries, found.page);
                }
            }
            // A failed suggestion is not worth a toast. Enter still searches.
            Err(_) if current => self.ui.set_suggest_loading(false),
            Err(_) => {}
        }
    }

    fn show_suggestions(
        self: &Rc<Self>,
        generation: u64,
        typed: &str,
        queries: Vec<String>,
        mut page: Page,
    ) {
        let mut rows = Vec::new();
        let mut picks = Vec::new();
        for query in queries.into_iter().take(5) {
            let (head, tail) = split_typed(&query, typed);
            rows.push(SuggestData {
                kind: 0,
                head: head.into(),
                tail: tail.into(),
                ..SuggestData::default()
            });
            picks.push(Pick::Query(query));
        }
        // The first results of the real search, tailored exactly as the
        // results page will be, so the list is a preview of that page.
        self.tailor_results(typed, &mut page, true);
        let mut items: Vec<Item> = Vec::new();
        for item in page.shelves.into_iter().flat_map(|s| s.items) {
            let key = history::key_of(&item);
            if key.is_some() && !items.iter().any(|i| history::key_of(i) == key) {
                items.push(item);
            }
        }
        for item in items.into_iter().take(4) {
            let row = rows.len();
            let mut data = self.item_data(&item, row, 64, Target::Suggest { generation, row });
            let subtitle = suggestion_subtitle(&item);
            // The artist named in the shorter line still links to its page.
            let (runs, _) = detail::link_runs(&subtitle, &item);
            data.subtitle = subtitle.into();
            data.runs = ModelRc::new(VecModel::from(runs));
            rows.push(SuggestData {
                kind: 1,
                item: data,
                ..SuggestData::default()
            });
            picks.push(Pick::Item(item));
        }
        let mut suggest = self.suggest.borrow_mut();
        suggest.picks = picks;
        suggest.rows.set_vec(rows);
        drop(suggest);
        self.ui.set_suggest_loading(false);
    }

    fn suggestion_picked(self: &Rc<Self>, index: usize) {
        let pick = self.suggest.borrow().picks.get(index).cloned();
        match pick {
            Some(Pick::Query(query)) => {
                self.ui.set_np_expanded(false);
                self.ui.set_search_text(query.as_str().into());
                self.suggest_edited(&query);
                self.remember_search(&query);
                self.open(Route::Search(query, 0), true);
            }
            Some(Pick::Item(item)) => {
                // A song plays without leaving expanded Now playing.
                if item.video_id.is_none() {
                    self.ui.set_np_expanded(false);
                }
                self.open_item(item)
            }
            None => {}
        }
    }

    /// Keeps a search at the top of the recent ones.
    fn remember_search(&self, query: &str) {
        let text = {
            let mut recent = self.recent_searches.borrow_mut();
            recent.retain(|q| !q.eq_ignore_ascii_case(query));
            recent.insert(0, query.to_string());
            recent.truncate(RECENT_SEARCHES_KEPT);
            recent.join("\n")
        };
        self.disk.run(move || {
            let _ = store::save(RECENT_SEARCHES, &text);
        });
    }

    /// Lists the recent searches under an empty search box.
    fn show_recent_searches(&self) {
        let recent = self.recent_searches.borrow().clone();
        let rows: Vec<SuggestData> = recent
            .iter()
            .map(|query| SuggestData {
                kind: 2,
                head: query.as_str().into(),
                ..SuggestData::default()
            })
            .collect();
        let mut suggest = self.suggest.borrow_mut();
        suggest.picks = recent.into_iter().map(Pick::Query).collect();
        suggest.rows.set_vec(rows);
        drop(suggest);
        self.ui.set_suggest_loading(false);
    }

    /// A tab on the search results page. It replaces the page in the
    /// history, so Back leaves the search rather than stepping through tabs.
    fn pick_search_tab(self: &Rc<Self>, tab: usize) {
        let current = self.history.borrow().last().cloned();
        let Some(Route::Search(query, _)) = current else {
            return;
        };
        let route = Route::Search(query, tab);
        if let Some(last) = self.history.borrow_mut().last_mut() {
            *last = route.clone();
        }
        self.open(route, false);
    }

    fn play_page(self: &Rc<Self>, shuffle: bool) {
        // Play on the page that is playing pauses and resumes it.
        if !shuffle && self.is_current(&self.page_key()) {
            self.toggle_play();
            return;
        }
        let songs: Vec<Item> = self
            .page_songs()
            .into_iter()
            .map(|(_, item)| item)
            .collect();
        // An artist page lists only five songs. Its own shuffle draws from
        // all of them, so shuffling, or playing with no list, uses that.
        let artist_shuffle = self
            .page
            .borrow()
            .header
            .as_ref()
            .filter(|h| h.kind == PageKind::Artist)
            .and_then(|h| h.shuffle.clone());
        if let Some(endpoint) = artist_shuffle.filter(|_| shuffle || songs.is_empty()) {
            self.play_endpoint(endpoint);
            return;
        }
        if songs.is_empty() {
            return;
        }
        let from = self.page_key();
        self.set_queue_from(if shuffle { shuffled(songs) } else { songs }, 0, from);
    }

    /// The key of the album, playlist or artist page that is open, if any.
    fn page_key(&self) -> String {
        match self.history.borrow().last() {
            Some(Route::Browse { id, .. }) => id.clone(),
            // Not a YouTube id, but mix mode and "playing from here" need
            // a key for the page all the same.
            Some(Route::Downloads) => DOWNLOADS_KEY.to_string(),
            _ => String::new(),
        }
    }

    /// Whether the item keyed `key` is the song playing or what the queue
    /// was started from.
    fn is_current(&self, key: &str) -> bool {
        if key.is_empty() {
            return false;
        }
        let player = self.player.borrow();
        let song = player.queue.get(player.index).and_then(|i| i.video_id.as_deref());
        song == Some(key) || *self.playing_from.borrow() == key
    }

    /// The play button on a card or row: plays the item at once, without
    /// opening its page. On what is already playing it pauses or resumes.
    fn item_play(self: &Rc<Self>, id: usize) {
        let Some(item) = self.page.borrow().items.get(id).cloned() else {
            return;
        };
        let key = history::key_of(&item).unwrap_or_default();
        if self.is_current(&key) && self.player.borrow().resume_at.is_none() {
            self.toggle_play();
            return;
        }
        if item.video_id.is_some() {
            self.activate(id);
            return;
        }
        self.remember(&item, 0.5);
        let client = self.client.clone();
        self.api.run(move || {
            // A playlist id plays albums, playlists and mixes alike. Others,
            // such as artists, play the songs listed on their page.
            let result = match (&item.playlist_id, &item.browse_id) {
                (Some(playlist), _) => client.queue(None, Some(playlist)).map(|(items, _)| items),
                (None, Some(browse)) => client
                    .browse(browse, item.params.as_deref())
                    .map(|page| songs_of(&page)),
                (None, None) => Ok(Vec::new()),
            };
            on_ui(move |app| match result {
                Ok(items) if !items.is_empty() => app.set_queue_from(items, 0, key),
                Ok(_) => app.toast("Nothing to play"),
                Err(message) => app.toast(message),
            });
        });
    }

    /// A stream that fails gets one more try with a fresh link, from where
    /// it stopped. Links can go stale or be refused once. Returns whether
    /// it is trying again.
    fn retry_stream(self: &Rc<Self>, message: &str) -> bool {
        if !message.starts_with(audio::STREAM_FAILED) {
            return false;
        }
        let (index, id) = {
            let player = self.player.borrow();
            let Some(id) = player.queue.get(player.index).and_then(|i| i.video_id.clone()) else {
                return false;
            };
            (player.index, id)
        };
        // One retry per song: the retry itself plays under a new generation.
        if self.retried.replace(Some(id.clone())).as_deref() == Some(id.as_str()) {
            return false;
        }
        let position = self.audio.status.position_ms.load(Ordering::SeqCst) as f32 / 1000.0;
        if let Some(resolver) = &self.resolver {
            resolver.forget(&id);
        }
        self.player.borrow_mut().resume_at = Some((id, position));
        self.play_index(index);
        true
    }

    /// Tells every play button what is playing.
    fn show_now(&self) {
        let now = self.ui.global::<Now>();
        let song = {
            let player = self.player.borrow();
            player
                .queue
                .get(player.index)
                .and_then(|i| i.video_id.clone())
                .unwrap_or_default()
        };
        now.set_song(song.into());
        now.set_from(self.playing_from.borrow().as_str().into());
        now.set_page(self.page_key().into());
    }

    // -------------------------------------------------------------- player

    fn set_queue(self: &Rc<Self>, items: Vec<Item>, index: usize) {
        self.set_queue_from(items, index, String::new());
    }

    /// Plays `items` from `index`, as started from the item keyed `from`.
    fn set_queue_from(self: &Rc<Self>, items: Vec<Item>, index: usize, from: String) {
        self.start_queue(items, index, from, true, None);
    }

    /// As `set_queue_from`. Without `shuffle` the songs keep their order
    /// even with shuffle on, and with `at` the first starts that many
    /// seconds in.
    fn start_queue(
        self: &Rc<Self>,
        items: Vec<Item>,
        index: usize,
        from: String,
        shuffle: bool,
        at: Option<f32>,
    ) {
        *self.playing_from.borrow_mut() = from;
        let first = items.get(index).and_then(|i| i.video_id.clone());
        let (items, index) = {
            let mut player = self.player.borrow_mut();
            if player.shuffle && shuffle {
                player.unshuffled = Some(items.clone());
                (shuffled_from(items, index), 0)
            } else {
                player.unshuffled = None;
                (items, index)
            }
        };
        {
            let mut player = self.player.borrow_mut();
            player.queue = Vec::new();
            player.queue_generation += 1;
            player.model = Rc::new(VecModel::default());
            player.extending = false;
            player.resume_at = first.zip(at);
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
        self.sync_up_next();
        self.save_session();
        if was_last {
            self.prefetch_next();
        }
    }

    /// Copies the rows after the playing one into "Next up".
    fn sync_up_next(&self) {
        let player = self.player.borrow();
        let rows: Vec<ItemData> = (player.index + 1..player.model.row_count())
            .filter_map(|i| player.model.row_data(i))
            .collect();
        player.up_next.set_vec(rows);
    }

    /// Moves a queued song from one place to another, by queue position.
    /// The playing song stays playing wherever it ends up.
    fn move_queued(self: &Rc<Self>, from: usize, to: usize) {
        {
            let mut player = self.player.borrow_mut();
            let len = player.queue.len();
            if from >= len || to >= len || from == to {
                return;
            }
            let item = player.queue.remove(from);
            player.queue.insert(to, item);
            player.index = index_after_move(player.index, from, to);
        }
        self.rebuild_queue_model();
        self.prefetch_next();
    }

    /// Asks for the radio of the last song and appends it, so music keeps
    /// going when the queue runs out.
    fn extend_with_radio(self: &Rc<Self>) {
        let (generation, last) = {
            let mut player = self.player.borrow_mut();
            if player.extending || player.repeat != Repeat::Off {
                return;
            }
            let Some(last) = player.queue.last().filter(|i| i.video_id.is_some()).cloned() else {
                return;
            };
            player.extending = true;
            (player.queue_generation, last)
        };
        let client = self.client.clone();
        self.api.run(move || {
            // A video's radio is videos. Seed it with the song instead.
            let seed = client
                .song_for(&last)
                .and_then(|s| s.video_id)
                .or(last.video_id)
                .unwrap_or_default();
            let result = client.queue(Some(&seed), None);
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
        let Some((item, generation, near_end)) = self.show_track(index) else {
            return;
        };
        // Saved before the resume point is used, so it is kept.
        self.save_session();
        let (resume, repeat) = {
            let mut player = self.player.borrow_mut();
            let resume = player
                .resume_at
                .take()
                .filter(|(at, _)| item.video_id.as_ref() == Some(at))
                .map(|(_, s)| s);
            (resume, player.repeat)
        };
        // Repeat loops the queue, so it does not grow a radio tail.
        if near_end && repeat == Repeat::Off {
            self.extend_with_radio();
        }
        let video_id = item.video_id.unwrap_or_default();
        // A downloaded or cached copy needs no yt-dlp and no network.
        if let Some(path) = offline::local_file(&video_id) {
            self.ui.set_buffering(true);
            self.load_audio(audio::Input::File(path), generation, resume);
            self.prefetch_next();
            return;
        }
        let Some(resolver) = self.resolver.clone() else {
            self.audio.stop();
            self.services.send(services::Event::Stopped);
            self.ui.set_buffering(false);
            self.toast("Playback is not set up yet. The README explains the missing piece.");
            return;
        };
        self.ui.set_buffering(true);
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
                        app.load_audio(
                            audio::Input::Http {
                                url: stream.url,
                                headers: stream.headers,
                                cache_as: Some(video_id),
                            },
                            generation,
                            resume,
                        );
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

    /// Starts a track, from `start` seconds in when given.
    fn load_audio(&self, input: audio::Input, generation: u64, start: Option<f32>) {
        // Loading drops whatever was opened ahead for a crossfade.
        self.fade_next.borrow_mut().take();
        self.audio.load(input, generation);
        self.apply_gain(generation);
        // Commands run in order, so the seek lands once the track is loaded.
        if let Some(seconds) = start.filter(|s| *s > 0.0) {
            self.audio.seek(seconds);
        }
    }

    /// Makes `index` the current song and shows it in the player, without
    /// starting the audio. Returns the song, its player generation, and
    /// whether the queue is about to run out.
    fn show_track(self: &Rc<Self>, index: usize) -> Option<(Item, u64, bool)> {
        let (item, generation, near_end) = {
            let mut player = self.player.borrow_mut();
            let item = player.queue.get(index).cloned()?;
            player.index = index;
            player.generation += 1;
            player.liked = false;
            player.duration = credits(&item).1;
            (item, player.generation, index + 2 >= player.queue.len())
        };
        let (artist, album, seconds) = credits_with_album(&item);
        let ui = &self.ui;
        ui.invoke_end_crossfade();
        self.outgoing.borrow_mut().take();
        ui.set_fade_motion_on(false);
        ui.set_track_album(album.into());
        ui.set_has_track(true);
        ui.set_track_title(item.title.as_str().into());
        ui.set_track_artist(artist.as_str().into());
        self.show_track_links(&item, &artist);
        ui.set_queue_index(index as i32);
        self.sync_up_next();
        ui.set_liked(false);
        ui.set_progress(0.0);
        ui.set_position_text(clock_text(0.0));
        ui.set_duration_text(clock_text(seconds));
        ui.set_playing(false);
        *self.cover.borrow_mut() = None;
        // Large enough for the Now playing panel. The player scales it down.
        let art = self.thumb(&item.thumb, 544, Target::Track { generation });
        if let Some(image) = &art {
            self.set_tint(image);
        }
        ui.set_track_has_art(art.is_some());
        match &art {
            Some(image) => self.backdrop_of(image),
            None => ui.set_track_backdrop(Image::default()),
        }
        ui.set_track_art(art.unwrap_or_default());
        self.load_cover(&item.title, &artist, generation);
        self.stop_canvas();
        self.load_canvas(&item.title, &artist, generation);
        self.load_credits(item.video_id.clone(), generation);
        if let Some(video_id) = &item.video_id {
            analysis::request(video_id);
        }
        self.show_sound();
        if matches!(ui.get_panel(), 2 | 3) {
            self.load_lyrics();
        }
        #[cfg(windows)]
        {
            self.update_keys(&item.title, &artist, seconds, &item.thumb);
            taskbar::set_song(&item.title, &artist);
        }
        self.show_now();
        Some((item, generation, near_end))
    }

    /// Fetches the song's credits for Now playing. They clear at once, so
    /// the last song's never show under this one.
    fn load_credits(&self, video_id: Option<String>, generation: u64) {
        self.ui.set_track_credits(ModelRc::default());
        let Some(video_id) = video_id else {
            return;
        };
        let client = self.client.clone();
        self.api.run(move || {
            let credits = client.credits(&video_id).unwrap_or_default();
            on_ui(move |app| {
                if app.player.borrow().generation != generation {
                    return;
                }
                let rows: Vec<CreditData> = credits
                    .into_iter()
                    .map(|(role, names)| CreditData {
                        role: role.into(),
                        names: names.into(),
                    })
                    .collect();
                app.ui.set_track_credits(ModelRc::new(VecModel::from(rows)));
            });
        });
    }

    // ------------------------------------------------------------- session

    /// Saves the queue, the playing song and how far into it you are, in
    /// the background.
    fn save_session(&self) {
        let text = {
            let player = self.player.borrow();
            if player.queue.is_empty() {
                return;
            }
            let status = &self.audio.status;
            let current = player.queue.get(player.index).and_then(|i| i.video_id.as_ref());
            let position = match &player.resume_at {
                // Restored and not played yet: keep the old place.
                Some((at, seconds)) if current == Some(at) => *seconds,
                _ if status.generation.load(Ordering::SeqCst) == player.generation => {
                    status.position_ms.load(Ordering::SeqCst) as f32 / 1000.0
                }
                _ => 0.0,
            };
            session::Session {
                queue: player.queue.clone(),
                index: player.index,
                position,
                volume: self.ui.get_volume(),
                shuffle: player.shuffle,
                repeat: player.repeat,
            }
            .serialize()
        };
        self.session_saved.set(Instant::now());
        self.disk.run(move || session::Session::save(text));
    }

    /// Puts back what was playing when the app last closed, paused.
    fn restore_session(self: &Rc<Self>) {
        let Some(saved) = session::Session::load() else {
            return;
        };
        self.ui.set_volume(saved.volume);
        self.audio.volume(saved.volume * saved.volume);
        self.ui.set_shuffle(saved.shuffle);
        self.ui.set_repeat(saved.repeat.number());
        {
            let mut player = self.player.borrow_mut();
            player.shuffle = saved.shuffle;
            player.repeat = saved.repeat;
            player.queue_generation += 1;
            self.ui.set_queue(ModelRc::from(player.model.clone()));
        }
        self.extend_queue(saved.queue);
        let Some((item, _, _)) = self.show_track(saved.index) else {
            return;
        };
        let duration = self.player.borrow().duration;
        // A song left in its last seconds starts over instead.
        let position = if duration > 0.0 && saved.position > duration - 5.0 {
            0.0
        } else {
            saved.position
        };
        if let Some(id) = item.video_id {
            self.show_resume_point(id, position);
        }
        // Filling the queue saved it before the place was known.
        self.save_session();
    }

    /// Sets where a restored song will start, and shows it on the timeline.
    fn show_resume_point(&self, id: String, seconds: f32) {
        let duration = self.player.borrow().duration;
        self.player.borrow_mut().resume_at = Some((id, seconds));
        self.ui.set_position_text(clock_text(seconds));
        self.ui.set_progress(if duration > 0.0 {
            (seconds / duration).min(1.0)
        } else {
            0.0
        });
    }

    /// Seeks the playing song. A restored song that has not started yet
    /// moves its start point instead.
    fn seek_to(&self, seconds: f32) {
        let pending = {
            let player = self.player.borrow();
            let current = player.queue.get(player.index).and_then(|i| i.video_id.clone());
            player
                .resume_at
                .as_ref()
                .filter(|(at, _)| current.as_ref() == Some(at))
                .map(|(at, _)| at.clone())
        };
        match pending {
            Some(id) => self.show_resume_point(id, seconds),
            None => self.audio.seek(seconds),
        }
    }

    // ------------------------------------------------------ shuffle, repeat

    /// Shuffles the songs after the playing one, or puts the queue back in
    /// its old order.
    fn toggle_shuffle(self: &Rc<Self>) {
        // Off, then shuffle, then smart shuffle, then off again.
        let smart = {
            let mut player = self.player.borrow_mut();
            if player.shuffle && !player.smart {
                player.smart = true;
                true
            } else {
                player.smart = false;
                false
            }
        };
        self.ui.set_smart_shuffle(smart);
        if smart {
            self.mix_in_suggestions();
            return;
        }
        self.take_out_suggestions();
        {
            let mut player = self.player.borrow_mut();
            player.shuffle = !player.shuffle;
            self.ui.set_shuffle(player.shuffle);
            let index = player.index;
            if player.shuffle {
                player.unshuffled = Some(player.queue.clone());
                if index + 1 < player.queue.len() {
                    let rest = player.queue.split_off(index + 1);
                    player.queue.extend(shuffled(rest));
                }
            } else if let Some(original) = player.unshuffled.take() {
                if let Some(current) = player.queue.get(index).cloned() {
                    let (queue, index) = unshuffle(original, &player.queue, &current);
                    player.queue = queue;
                    player.index = index;
                }
            }
        }
        self.rebuild_queue_model();
        self.prefetch_next();
    }

    fn cycle_repeat(self: &Rc<Self>) {
        let repeat = {
            let mut player = self.player.borrow_mut();
            player.repeat = player.repeat.next();
            player.repeat
        };
        self.ui.set_repeat(repeat.number());
        self.save_session();
        // Turning repeat off near the end lets the radio carry on.
        let near_end = {
            let player = self.player.borrow();
            player.index + 2 >= player.queue.len()
        };
        if repeat == Repeat::Off && near_end {
            self.extend_with_radio();
        }
    }

    /// A song played to its end: again under repeat one, the next one
    /// otherwise.
    fn track_ended(self: &Rc<Self>) {
        if self.sleep.get() == Sleep::EndOfSong {
            self.sleep_done();
            return;
        }
        let (index, repeat) = {
            let player = self.player.borrow();
            (player.index, player.repeat)
        };
        if repeat == Repeat::One {
            self.play_index(index);
        } else {
            self.next();
        }
    }

    // ---------------------------------------------------- smart shuffle

    /// Mixes songs like the queue into what is still to come, one after
    /// every few, the way smart shuffle does.
    fn mix_in_suggestions(self: &Rc<Self>) {
        let (seed, generation) = {
            let player = self.player.borrow();
            let seed = player.queue.get(player.index).cloned();
            (seed, player.queue_generation)
        };
        let Some(seed) = seed.filter(|s| s.video_id.is_some()) else {
            return;
        };
        let client = self.client.clone();
        self.api.run(move || {
            // A video's radio is videos. Seed it with the song instead.
            let id = client
                .song_for(&seed)
                .and_then(|s| s.video_id)
                .or(seed.video_id)
                .unwrap_or_default();
            let result = client.queue(Some(&id), None);
            on_ui(move |app| {
                let Ok((items, _)) = result else {
                    app.toast("Smart shuffle found no songs to add");
                    return;
                };
                {
                    let mut player = app.player.borrow_mut();
                    if player.queue_generation != generation || !player.smart {
                        return;
                    }
                    let known: HashSet<String> =
                        player.queue.iter().filter_map(|i| i.video_id.clone()).collect();
                    let picks: Vec<Item> = items
                        .into_iter()
                        .filter(|i| i.video_id.as_ref().is_some_and(|v| !known.contains(v)))
                        .take(SMART_MOST)
                        .collect();
                    let mut at = player.index + 1;
                    for item in picks {
                        at = (at + SMART_EVERY).min(player.queue.len());
                        if let Some(id) = item.video_id.clone() {
                            player.suggested.insert(id);
                        }
                        player.queue.insert(at, item);
                        at += 1;
                    }
                }
                app.rebuild_queue_model();
                app.prefetch_next();
            });
        });
    }

    /// Takes out the songs smart shuffle mixed in that are still to come.
    fn take_out_suggestions(self: &Rc<Self>) {
        let changed = {
            let mut player = self.player.borrow_mut();
            let suggested = std::mem::take(&mut player.suggested);
            if suggested.is_empty() {
                false
            } else {
                let index = player.index;
                let mut position = 0;
                let before = player.queue.len();
                player.queue.retain(|item| {
                    let keep = position <= index
                        || !item.video_id.as_ref().is_some_and(|v| suggested.contains(v));
                    position += 1;
                    keep
                });
                before != player.queue.len()
            }
        };
        if changed {
            self.rebuild_queue_model();
        }
    }

    // ------------------------------------------------------- shortcuts

    fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.ui.set_volume(volume);
        self.audio.volume(volume * volume);
    }

    /// A key pressed with no text box focused. Returns whether it was one
    /// of the shortcuts, the same ones Spotify uses.
    fn hotkey(self: &Rc<Self>, text: &str, control: bool, shift: bool, alt: bool) -> bool {
        use slint::platform::Key;
        let key = |k: Key| text == SharedString::from(k).as_str();
        match (control, shift, alt) {
            (false, false, false) if text == " " => self.toggle_play(),
            (false, false, false) if key(Key::Escape) => {
                if self.ui.get_prompt_open() {
                    self.ui.set_prompt_open(false);
                } else if self.ui.get_np_expanded() {
                    self.ui.set_np_expanded(false);
                } else {
                    return false;
                }
            }
            (true, false, false) if key(Key::RightArrow) => self.next(),
            (true, false, false) if key(Key::LeftArrow) => self.previous(),
            (true, false, false) if key(Key::UpArrow) => self.set_volume(self.ui.get_volume() + 0.1),
            (true, false, false) if key(Key::DownArrow) => self.set_volume(self.ui.get_volume() - 0.1),
            (true, false, false) if text.eq_ignore_ascii_case("s") => self.toggle_shuffle(),
            (true, false, false) if text.eq_ignore_ascii_case("r") => self.cycle_repeat(),
            (true, false, false) if text.eq_ignore_ascii_case("l") || text.eq_ignore_ascii_case("k") => {
                self.ui.invoke_focus_search();
            }
            (false, true, true) if text.eq_ignore_ascii_case("b") => self.toggle_like(),
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------ sleep timer

    /// A sleep timer choice from the player: 0 is off, then the minutes in
    /// `SLEEP_MINUTES`, then the end of the song.
    fn set_sleep(self: &Rc<Self>, choice: i32) {
        let sleep = match choice {
            SLEEP_END_OF_SONG => Sleep::EndOfSong,
            n if n >= 1 => SLEEP_MINUTES
                .get(n as usize - 1)
                .map_or(Sleep::Off, |m| Sleep::At(Instant::now() + Duration::from_secs(m * 60))),
            _ => Sleep::Off,
        };
        self.sleep.set(sleep);
        self.ui.set_sleep_choice(if sleep == Sleep::Off { 0 } else { choice });
        // A song armed to follow this one is called off by the next tick.
        self.sleep_tick();
    }

    /// Shows the time left, and pauses once it is up.
    fn sleep_tick(self: &Rc<Self>) {
        let text = match self.sleep.get() {
            Sleep::Off => String::new(),
            Sleep::EndOfSong => "End of song".into(),
            Sleep::At(at) => {
                let left = at.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    self.audio.pause();
                    self.sleep_done();
                    return;
                }
                format!("{} min left", (left.as_secs() + 59) / 60)
            }
        };
        if self.ui.get_sleep_text().as_str() != text {
            self.ui.set_sleep_text(text.into());
        }
    }

    /// The sleep timer ran out. The music has stopped or is pausing.
    fn sleep_done(self: &Rc<Self>) {
        self.sleep.set(Sleep::Off);
        self.ui.set_sleep_choice(0);
        self.ui.set_sleep_text("".into());
        self.ui.set_playing(false);
        self.toast("Sleep timer stopped the music");
    }

    // ------------------------------------------------------ levelling

    fn set_levelling(&self, choice: i32) {
        let choice = choice.clamp(0, LEVELLING.len() as i32 - 1);
        self.levelling.set(choice);
        self.disk.run(move || {
            let _ = store::save("levelling", &choice.to_string());
        });
        let generation = self.player.borrow().generation;
        self.apply_gain(generation);
    }

    /// The gain that brings `video_id` to the chosen loudness. One while
    /// levelling is off or its loudness is not known yet.
    fn gain_for(&self, video_id: &str) -> f32 {
        let choice = self.levelling.get();
        let Some(&(target, lift)) = LEVELLING.get(choice as usize).filter(|_| choice > 0) else {
            return 1.0;
        };
        let Some(lkfs) = self.loudness.borrow().get(video_id).copied() else {
            return 1.0;
        };
        let db = (target - lkfs).clamp(-20.0, lift);
        10f32.powf(db / 20.0)
    }

    /// Sends the playing song's levelling gain to the engine, and looks its
    /// loudness up when that is not known yet.
    fn apply_gain(&self, generation: u64) {
        let video_id = {
            let player = self.player.borrow();
            if player.generation != generation {
                return;
            }
            player.queue.get(player.index).and_then(|i| i.video_id.clone())
        };
        let Some(video_id) = video_id else {
            return;
        };
        self.look_up_loudness(&video_id);
        self.audio.gain(generation, self.gain_for(&video_id));
    }

    /// Asks YouTube for a song's loudness in the background, once. When the
    /// answer is for the playing song, its gain is set at once.
    fn look_up_loudness(&self, video_id: &str) {
        if self.loudness.borrow().contains_key(video_id)
            || !self.loudness_asked.borrow_mut().insert(video_id.to_string())
        {
            return;
        }
        let client = self.client.clone();
        let video_id = video_id.to_string();
        self.warm.run(move || {
            let found = client.loudness(&video_id).ok().flatten();
            on_ui(move |app| {
                let Some(lkfs) = found else {
                    return;
                };
                app.loudness.borrow_mut().insert(video_id, lkfs);
                let generation = app.player.borrow().generation;
                app.apply_gain(generation);
            });
        });
    }

    // ------------------------------------------------------------ crossfade

    fn set_crossfade_choice(&self, choice: i32) {
        let seconds = CROSSFADE_CHOICES
            .get(choice.max(0) as usize)
            .copied()
            .unwrap_or(0);
        self.crossfade.set(seconds);
        self.disk.run(move || {
            let _ = store::save("crossfade", &seconds.to_string());
        });
        self.forget_plan();
    }

    /// A setting changed, so the mix into the next song is worked out again.
    fn forget_plan(&self) {
        self.planned.borrow_mut().take();
    }

    // ------------------------------------------------------------- analysis

    /// A song's analysis, once the analyzer has been through it.
    fn analysis_of(&self, video_id: &str) -> Option<Rc<analysis::Analysis>> {
        if let Some(found) = self.analyses.borrow().get(video_id) {
            return Some(found.clone());
        }
        if self.unanalyzed.borrow().contains(video_id) {
            return None;
        }
        match library::analysis(video_id) {
            Some(found) => {
                let found = Rc::new(found);
                self.analyses.borrow_mut().insert(video_id.to_string(), found.clone());
                Some(found)
            }
            None => {
                self.unanalyzed.borrow_mut().insert(video_id.to_string());
                None
            }
        }
    }

    /// The analyzer started on a song.
    fn analysis_started(&self, _video_id: &str) {
        if self.ui.get_header().mixing {
            self.refresh_mix_rows();
        }
    }

    /// The analyzer finished a song, or gave up on it.
    fn analysis_ready(&self, video_id: &str) {
        self.unanalyzed.borrow_mut().remove(video_id);
        // The mix may now be beat matched.
        let involved = self
            .planned
            .borrow()
            .as_ref()
            .is_some_and(|p| p.from == video_id || p.into == video_id);
        if involved {
            self.forget_plan();
        }
        if self.playing_id().as_deref() == Some(video_id) {
            self.show_sound();
        }
        if self.ui.get_header().mixing {
            self.refresh_mix_rows();
        }
    }

    fn playing_id(&self) -> Option<String> {
        let player = self.player.borrow();
        player.queue.get(player.index)?.video_id.clone()
    }

    /// Shows the playing song's tempo and key in Now playing, once known.
    fn show_sound(&self) {
        let found = self.playing_id().and_then(|id| self.analysis_of(&id));
        let mut rows = Vec::new();
        if let Some(a) = found {
            if a.bpm > 0.0 {
                let bpm = format!("{:.0} BPM", a.bpm);
                let names = if a.steady() { bpm } else { format!("About {bpm}, and it moves") };
                rows.push(CreditData { role: "Tempo".into(), names: names.into() });
            }
            if let (Some(camelot), Some(name)) = (a.camelot(), a.key_name()) {
                if a.key_confidence >= 0.4 {
                    rows.push(CreditData { role: "Key".into(), names: format!("{name} · {camelot}").into() });
                }
            }
        }
        self.ui.set_track_sound(ModelRc::new(VecModel::from(rows)));
    }

    // ------------------------------------------------------------ crossfade

    /// The queue position and video id of the song the playing one runs
    /// into at its end, when that can be a crossfade. Never under repeat
    /// one, where the song plays again.
    fn crossfade_target(&self) -> Option<(usize, String)> {
        let player = self.player.borrow();
        // The sleep timer stops after this song, so nothing follows it.
        if player.repeat == Repeat::One || self.sleep.get() == Sleep::EndOfSong {
            return None;
        }
        // The same choice `next` makes.
        let mut next = player.index + 1;
        if next >= player.queue.len() {
            if player.repeat == Repeat::Off {
                return None;
            }
            next = 0;
        }
        if next == player.index {
            return None;
        }
        let video_id = player.queue.get(next)?.video_id.clone()?;
        Some((next, video_id))
    }


    /// Whether the queue was started from a playlist with mix mode on.
    fn mix_mode(&self) -> bool {
        self.mixed_playlists.borrow().contains(self.playing_from.borrow().as_str())
    }

    /// About how long mixes into the next song last, in seconds. Zero when
    /// songs do not mix: gapless. Mix mode mixes even with crossfade off.
    fn mix_seconds(&self) -> f32 {
        match self.crossfade.get() {
            0 if self.mix_mode() => MIX_MODE_SECONDS,
            seconds => seconds as f32,
        }
    }

    /// The transition from `from` into `to`: one being previewed from the
    /// editor, the one chosen by hand, or Auto's pick for the two songs.
    fn transition_settings(&self, from: &str, to: &str) -> transition::Settings {
        if let Some((a, b, settings)) = self.previewing.borrow().as_ref() {
            if a == from && b == to {
                return *settings;
            }
        }
        library::transition(from, to).unwrap_or_else(|| {
            let out = self.analysis_of(from);
            let inn = self.analysis_of(to);
            transition::Settings::auto(out.as_deref(), inn.as_deref(), self.mix_mode())
        })
    }

    /// The mix from the playing song into the next one. Beat matched once
    /// both songs are analyzed, usually soon after the song starts, as the
    /// next one is fetched ahead. Kept until the songs or a setting change.
    fn mix_plan(&self, next_id: &str, duration: f32) -> Option<(transition::Settings, mix::Transition)> {
        let seconds = self.mix_seconds();
        if seconds <= 0.0 {
            return None;
        }
        let playing = self.playing_id()?;
        if let Some(p) = self.planned.borrow().as_ref() {
            if p.from == playing && p.into == next_id {
                return Some((p.settings, p.plan.clone()));
            }
        }
        let settings = self.transition_settings(&playing, next_id);
        let out = self.analysis_of(&playing);
        let inn = self.analysis_of(next_id);
        let plan = transition::plan(settings, out.as_deref(), duration as f64, inn.as_deref(), seconds as f64);
        *self.planned.borrow_mut() = Some(Planned {
            from: playing,
            into: next_id.to_string(),
            settings,
            plan: plan.clone(),
        });
        Some((settings, plan))
    }

    /// Where the mix into the next song starts, in seconds of the playing
    /// one. None when nothing will mix in.
    fn mix_start(&self, duration: f32) -> Option<f32> {
        let (_, next_id) = self.crossfade_target()?;
        let (_, plan) = self.mix_plan(&next_id, duration)?;
        let start = plan.out_at.map_or(duration - plan.length as f32, |at| at as f32);
        (start > duration * 0.25).then_some(start)
    }

    /// Called each tick while a song plays. Opens the next song ahead of
    /// the mix, and once the playing song reaches it, mixes into it.
    /// Returns whether it moved on to the next song.
    ///
    /// If the next song is not open in time, nothing happens here: the
    /// song ends and the next one starts the usual way.
    fn crossfade_check(self: &Rc<Self>, generation: u64, duration: f32) -> bool {
        if duration <= 0.0 {
            return false;
        }
        let Some((index, video_id)) = self.crossfade_target() else {
            // Nothing should follow any more, as under repeat one.
            if self.fade_next.borrow_mut().take().is_some_and(|n| n.armed) {
                self.audio.unfollow();
            }
            return false;
        };
        let status = &self.audio.status;
        let position = status.position_ms.load(Ordering::SeqCst) as f32 / 1000.0;
        let left = duration - position;
        let planned = self.mix_plan(&video_id, duration);
        let opened = self
            .fade_next
            .borrow()
            .as_ref()
            .filter(|n| n.generation == generation && n.index == index && n.video_id == video_id)
            .map(|n| (n.token, n.duration, n.armed));
        let Some((token, next_duration, armed)) = opened else {
            // Also opens it again when the queue changed underneath. A beat
            // matched mix may start well before the end, and one not yet
            // planned needs time to fetch and analyze the next song.
            let ahead = match planned.as_ref().map(|(_, p)| (p.out_at, p.length)) {
                Some((Some(at), _)) => duration - at as f32 + PREPARE_AHEAD,
                Some((None, length)) if self.mix_mode() => {
                    length as f32 + PREPARE_AHEAD_MIXING
                }
                Some((None, length)) => length as f32 + PREPARE_AHEAD,
                None => PREPARE_AHEAD,
            };
            if left <= ahead {
                self.prepare_next(generation, index, video_id);
            }
            return false;
        };
        // Mixing off: gapless. The engine starts the next song the moment
        // this one ends, and `tick` catches up when it has.
        let Some((settings, plan)) = planned else {
            if !armed && status.prepared.load(Ordering::SeqCst) == token {
                self.look_up_loudness(&video_id);
                self.audio.follow(token, self.gain_for(&video_id));
                if let Some(next) = self.fade_next.borrow_mut().as_mut() {
                    next.armed = true;
                }
            }
            return false;
        };
        let ready = status.prepared.load(Ordering::SeqCst) == token;
        // Beat matched: handed over just before its downbeat, and the
        // engine starts it to the frame.
        if let Some(at) = plan.out_at {
            let at = at as f32;
            if position < at - MIX_LEAD {
                return false;
            }
            if ready && position <= at {
                self.crossfade_into(index, token, plan, next_duration);
                return true;
            }
            // Reached too late, as after a seek: the same style, unmatched.
        }
        // A cut not on the beat comes at the very end of the song, which is
        // a gapless start. The engine makes that to the frame. A tick could
        // not, and missed it, so the next song loaded afresh after a gap.
        if settings.volume == transition::Volume::Cut {
            if !armed && ready {
                self.look_up_loudness(&video_id);
                self.audio.follow(token, self.gain_for(&video_id));
                if let Some(next) = self.fade_next.borrow_mut().as_mut() {
                    next.armed = true;
                }
            }
            return false;
        }
        let length = plan.length as f32;
        if left > length || !ready || left < SHORTEST_FADE.min(length) {
            return false;
        }
        // Reached late, it mixes over what is left rather than cutting the
        // old song short.
        let mut unmatched = settings;
        unmatched.beatmatch = false;
        let plan = transition::plan(unmatched, None, duration as f64, None, left.min(length) as f64);
        self.crossfade_into(index, token, plan, next_duration);
        true
    }

    /// Opens the song at `index` in the audio engine, ready to fade in.
    fn prepare_next(self: &Rc<Self>, generation: u64, index: usize, video_id: String) {
        let token = self.fade_token.get() + 1;
        self.fade_token.set(token);
        *self.fade_next.borrow_mut() = Some(FadeNext {
            token,
            generation,
            index,
            video_id: video_id.clone(),
            duration: 0.0,
            armed: false,
        });
        if let Some(path) = offline::local_file(&video_id) {
            analysis::request(&video_id);
            self.audio.prepare(audio::Input::File(path), token);
            return;
        }
        let Some(resolver) = self.resolver.clone() else {
            return;
        };
        self.api.run(move || {
            // Usually at once: `prefetch_next` got the link when the
            // playing song started.
            let result = resolver.resolve(&video_id);
            on_ui(move |app| {
                let mut next = app.fade_next.borrow_mut();
                let Some(next) = next.as_mut().filter(|n| n.token == token) else {
                    return;
                };
                match result {
                    Ok(stream) => {
                        next.duration = stream.duration;
                        app.audio.prepare(
                            audio::Input::Http {
                                url: stream.url,
                                headers: stream.headers,
                                cache_as: Some(video_id),
                            },
                            token,
                        );
                    }
                    // Left in place, so it is not tried again every tick.
                    Err(message) => eprintln!("crossfade: {message}"),
                }
            });
        });
    }

    /// Mixes into the prepared song at `index` by `plan`. The player, Now
    /// playing, lyrics and media keys switch to it as the mix starts, the
    /// same way as when a song starts.
    fn crossfade_into(self: &Rc<Self>, index: usize, token: u64, plan: mix::Transition, duration: f32) {
        // The song going out stays on show and fades as its sound does,
        // its Canvas or moving cover still playing.
        let ui = &self.ui;
        ui.set_fade_title(ui.get_track_title());
        ui.set_fade_artist(ui.get_track_artist());
        ui.set_fade_art(ui.get_track_art());
        ui.set_fade_has_art(ui.get_track_has_art());
        ui.set_fade_motion(ui.get_track_motion());
        let motion_on = ui.get_track_motion_on();
        let cover = self.cover.borrow_mut().take();
        let canvas = self.canvas.borrow_mut().take();
        // The expanded view's own clip ends with its song.
        self.wide.borrow_mut().take();
        ui.set_wide_motion_on(false);
        let seconds = plan.length as f32;
        let video_id = {
            let player = self.player.borrow();
            player.queue.get(player.index).and_then(|i| i.video_id.clone()).unwrap_or_default()
        };
        self.moved_on(index, duration, |app, generation| {
            app.audio.crossfade(token, generation, plan);
        });
        // After the move, which ends any crossfade still going.
        *self.outgoing.borrow_mut() = Some(Outgoing {
            cover,
            canvas,
            video_id,
            started: Instant::now(),
            until: Instant::now() + Duration::from_secs_f32(seconds),
        });
        self.ui.set_fade_motion_on(motion_on);
        self.ui.invoke_begin_crossfade(seconds);
    }

    /// The engine started the armed next song gaplessly. Moves the player
    /// on to it, as a crossfade would.
    fn followed(self: &Rc<Self>, token: u64) {
        let next = self.fade_next.borrow().as_ref().filter(|n| n.token == token).map(|n| (n.index, n.duration));
        let Some((index, duration)) = next else {
            return;
        };
        self.moved_on(index, duration, |app, generation| app.audio.adopt(generation));
    }

    /// Shows the song at `index`, which `start` has just begun playing
    /// under its new generation, and gets the one after it ready.
    fn moved_on(self: &Rc<Self>, index: usize, duration: f32, start: impl FnOnce(&Rc<Self>, u64)) {
        self.fade_next.borrow_mut().take();
        let Some((_, generation, near_end)) = self.show_track(index) else {
            return;
        };
        start(self, generation);
        self.apply_gain(generation);
        if duration > 0.0 {
            self.player.borrow_mut().duration = duration;
            self.ui.set_duration_text(clock_text(duration));
        }
        self.save_session();
        if near_end && self.player.borrow().repeat == Repeat::Off {
            self.extend_with_radio();
        }
        self.prefetch_next();
    }

    /// Gets a song's stream link in the background, so playing it starts
    /// at once. YouTube's wait on a fresh link counts from when it was
    /// given out, so getting it early hides that wait too. Skips songs
    /// saved on this computer, and gives up when enough are already going.
    fn warm_link(&self, video_id: String) {
        if offline::local_file(&video_id).is_some() {
            return;
        }
        let Some(resolver) = self.resolver.clone() else {
            return;
        };
        {
            let mut warming = self.warming.borrow_mut();
            if warming.len() >= WARM_AT_ONCE || !warming.insert(video_id.clone()) {
                return;
            }
        }
        self.warm.run(move || {
            // Cached, so a link already got returns at once.
            resolver.prefetch(&video_id);
            on_ui(move |app| {
                app.warming.borrow_mut().remove(&video_id);
            });
        });
    }

    /// The mouse rests on a song on the page: it is likely to be played.
    fn item_rested(&self, id: usize) {
        let item = self.page.borrow().items.get(id).cloned();
        // A video plays as its album song, which takes a lookup to find.
        // Albums and playlists play from their first song once opened.
        if let Some(item) = item.filter(|i| !innertube::is_video(i)) {
            if let Some(video_id) = item.video_id {
                self.warm_link(video_id);
            }
        }
    }

    /// Fetches the next track into the offline cache in the background, so
    /// skipping to it, or reaching it, starts at once. With the cache off,
    /// it still resolves the stream link to skip the yt-dlp wait.
    fn prefetch_next(self: &Rc<Self>) {
        let (next, after) = {
            let player = self.player.borrow();
            let id = |n: usize| player.queue.get(n).and_then(|item| item.video_id.clone());
            (id(player.index + 1), id(player.index + 2))
        };
        // The song after next only gets its link, which costs no download.
        if let Some(video_id) = after {
            self.warm_link(video_id);
        }
        let Some(video_id) = next else {
            return;
        };
        self.look_up_loudness(&video_id);
        if offline::cache_limit_mb() > 0 {
            self.cache_ahead(video_id);
        } else if let Some(resolver) = self.resolver.clone() {
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

    /// Finds what each view shows for the cover, as Settings say: the
    /// track's Spotify Canvas, or the album's moving cover from Apple Music,
    /// or the still cover. Each view falls back to the still cover when its
    /// clip cannot be had. A clip both views show plays once for both.
    fn load_canvas(self: &Rc<Self>, title: &str, artist: &str, generation: u64) {
        let (panel, wide) = self.cover_choices.get();
        let keys = canvas_keys(&self.settings.borrow());
        // Set by show_track just before.
        let album = self.ui.get_track_album().to_string();
        let want_canvas = panel == PANEL_CANVAS && !keys.cookie.is_empty() && !title.is_empty();
        let want_apple =
            (panel == PANEL_ANIMATED || wide == WIDE_ANIMATED) && !(album.is_empty() && title.is_empty());
        if !want_canvas && !want_apple {
            return;
        }
        let (http, title, artist) = (self.http.clone(), title.to_string(), artist.to_string());
        self.api.run(move || {
            let apple = if want_apple {
                animated_art::fetch(&http, &album, &title, &artist).unwrap_or_else(|message| {
                    eprintln!("animated cover: {message}");
                    None
                })
            } else {
                None
            };
            let spotify = if want_canvas {
                canvas::fetch(&http, &keys, &title, &artist).unwrap_or_else(|message| {
                    eprintln!("canvas: {message}");
                    None
                })
            } else {
                None
            };
            on_ui(move |app| {
                if app.player.borrow().generation != generation {
                    return;
                }
                // The choices as they are now, which may have changed meanwhile.
                let (panel, wide) = app.cover_choices.get();
                let apple_in_panel = panel == PANEL_ANIMATED && apple.is_some();
                let apple_in_wide = wide == WIDE_ANIMATED && apple.is_some();
                if let Some(path) = apple.filter(|_| apple_in_panel || apple_in_wide) {
                    let motion = app.start_clip(path, animated_art::WIDTH, apple_in_panel, apple_in_wide);
                    if apple_in_panel {
                        *app.canvas.borrow_mut() = Some(motion);
                    } else {
                        *app.wide.borrow_mut() = Some(motion);
                    }
                }
                if let Some(path) = spotify.filter(|_| panel == PANEL_CANVAS) {
                    let motion = app.start_clip(path, canvas::WIDTH, true, false);
                    *app.canvas.borrow_mut() = Some(motion);
                }
            });
        });
    }

    /// Plays the clip at `path`, `width` pixels wide, in the views asked for.
    fn start_clip(self: &Rc<Self>, path: std::path::PathBuf, width: u32, panel: bool, wide: bool) -> Motion {
        let id = self.clips.get() + 1;
        self.clips.set(id);
        let clip = canvas::Clip::play_at(
            path,
            width,
            move |pixels, _blur| {
                on_ui(move |app| {
                    let live = |slot: &RefCell<Option<Motion>>| slot.borrow().as_ref().is_some_and(|m| m.id == id);
                    if live(&app.canvas) || live(&app.wide) {
                        let image = Image::from_rgba8(pixels);
                        if panel {
                            app.ui.set_track_motion(image.clone());
                            app.ui.set_track_motion_on(true);
                        }
                        if wide {
                            app.ui.set_wide_motion(image);
                            app.ui.set_wide_motion_on(true);
                        }
                    } else if panel
                        && app
                            .outgoing
                            .borrow()
                            .as_ref()
                            .and_then(|o| o.canvas.as_ref())
                            .is_some_and(|m| m.id == id)
                    {
                        // Fading out under the next song.
                        app.ui.set_fade_motion(Image::from_rgba8(pixels));
                    }
                })
            },
            move |message| {
                eprintln!("canvas: {message}");
                // Back to the still cover in the views it showed in.
                on_ui(move |app| {
                    for slot in [&app.canvas, &app.wide] {
                        if slot.borrow().as_ref().is_some_and(|m| m.id == id) {
                            slot.borrow_mut().take();
                            if panel {
                                app.ui.set_track_motion_on(false);
                            }
                            if wide {
                                app.ui.set_wide_motion_on(false);
                            }
                        }
                    }
                });
            },
        );
        Motion { id, clip, panel, wide }
    }

    /// Changes what a view shows for the cover, and starts the playing
    /// song's clips again to match.
    fn set_cover_choice(self: &Rc<Self>, panel: i32, wide: i32) {
        let choices = (panel.clamp(0, 2), wide.clamp(0, 1));
        if choices == self.cover_choices.get() {
            return;
        }
        self.cover_choices.set(choices);
        self.disk.run(move || {
            let _ = store::save("cover-choices", &format!("{},{}", choices.0, choices.1));
        });
        self.stop_canvas();
        let (title, artist, generation) = {
            let player = self.player.borrow();
            let Some(item) = player.queue.get(player.index) else { return };
            (item.title.clone(), credits(item).0, player.generation)
        };
        self.load_canvas(&title, &artist, generation);
    }

    /// Stops the Canvas and fades back to the still cover underneath.
    fn stop_canvas(&self) {
        // Dropping the clips stops their threads.
        self.canvas.borrow_mut().take();
        self.wide.borrow_mut().take();
        self.ui.set_track_motion_on(false);
        self.ui.set_wide_motion_on(false);
        let still = self.ui.get_track_art();
        self.backdrop_of(&still);
    }

    /// Fills the expanded view's background with a blur of `image`.
    fn backdrop_of(&self, image: &Image) {
        match image.to_rgba8() {
            Some(pixels) => {
                let base = blur::backdrop(&pixels);
                self.ui.set_track_backdrop(Image::from_rgba8(base.clone()));
                *self.backdrop_base.borrow_mut() = Some(base);
            }
            None => {
                self.ui.set_track_backdrop(Image::default());
                *self.backdrop_base.borrow_mut() = None;
            }
        }
    }

    /// Starts or stops the background's waves, as expanded Now playing
    /// opens and closes. About 30 small frames a second, made here, as
    /// moving big images in the window costs far more.
    fn wave(self: &Rc<Self>) {
        let on = self.ui.get_np_expanded() && !self.hidden.get() && !self.minimized.get();
        if on == self.wave_timer.running() {
            return;
        }
        if !on {
            self.wave_timer.stop();
            return;
        }
        let weak = Rc::downgrade(self);
        let start = Instant::now();
        self.wave_timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(app) = weak.upgrade() else { return };
            let frame = app.backdrop_base.borrow().as_ref().map(|base| blur::wave(base, start.elapsed().as_secs_f32()));
            if let Some(frame) = frame {
                app.ui.set_track_backdrop(Image::from_rgba8(frame));
            }
        });
    }

    /// Shows the playing track's still cover again.
    fn restore_still_cover(self: &Rc<Self>) {
        let (thumb, generation) = {
            let player = self.player.borrow();
            let Some(item) = player.queue.get(player.index) else { return };
            (item.thumb.clone(), player.generation)
        };
        if let Some(image) = self.thumb(&thumb, 544, Target::Track { generation }) {
            self.backdrop_of(&image);
            self.ui.set_track_art(image);
            self.ui.set_track_has_art(true);
        }
    }

    #[cfg(windows)]
    fn update_keys(self: &Rc<Self>, title: &str, artist: &str, seconds: f32, thumb: &str) {
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
            keys.track(title, artist, seconds, thumb);
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
        let index = {
            let player = self.player.borrow();
            let next = player.index + 1;
            if next >= player.queue.len() && player.repeat != Repeat::Off {
                0
            } else {
                next
            }
        };
        self.play_index(index);
    }

    fn previous(self: &Rc<Self>) {
        let (index, len, repeat) = {
            let player = self.player.borrow();
            (player.index, player.queue.len(), player.repeat)
        };
        let wrap = index == 0 && repeat != Repeat::Off && len > 1;
        if self.audio.status.position_ms.load(Ordering::SeqCst) > 3000 || (index == 0 && !wrap) {
            self.audio.seek(0.0);
        } else if wrap {
            self.play_index(len - 1);
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
        self.ui.set_lyrics_preview(ModelRc::default());
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
            on_ui(move |app| {
                app.show_lyrics(generation, found);
                app.translate_lyrics();
            });
        });
    }

    /// The lyrics' space changed width, as when Now playing expands: break
    /// the lines again to fit.
    fn rewrap_lyrics(&self) {
        let (generation, found) = {
            let state = self.lyrics.borrow();
            (state.generation, state.found.clone())
        };
        if found.is_some() {
            self.show_lyrics(generation, found);
        }
    }

    fn set_translate_choice(&self, choice: i32) {
        let index = (choice.max(0) as usize).min(TRANSLATE_CHOICES.len() - 1);
        self.translate.set(index);
        let tag = TRANSLATE_CHOICES[index];
        self.disk.run(move || {
            let _ = store::save("translate", tag);
        });
        self.translate_lyrics();
    }

    /// Puts translations under the lyrics on show, in the language picked in
    /// Settings, or takes them away when it is off. The source's own
    /// translation is used when it has one in that language; otherwise the
    /// lines are machine translated in the background.
    fn translate_lyrics(&self) {
        let target = TRANSLATE_CHOICES[self.translate.get()];
        let (generation, mut found) = {
            let mut state = self.lyrics.borrow_mut();
            let Some(found) = state.found.clone() else { return };
            if state.translated_to == target {
                return;
            }
            state.translated_to = target;
            (state.generation, found)
        };
        if target.is_empty() {
            for line in &mut found.lines {
                line.translation.clear();
            }
            self.show_lyrics(generation, Some(found));
            return;
        }
        // Lines with a translation of their own in the language use it. Any
        // older translation goes while the rest come.
        let mut missing: Vec<usize> = Vec::new();
        for (index, line) in found.lines.iter_mut().enumerate() {
            let official = line
                .official
                .iter()
                .find(|(lang, _)| lyrics::same_language(lang, target))
                .map(|(_, text)| text.clone());
            line.translation = match official {
                Some(text) if text != line.text => text,
                Some(_) => String::new(),
                None => {
                    missing.push(index);
                    String::new()
                }
            };
        }
        self.show_lyrics(generation, Some(found.clone()));
        if missing.is_empty() {
            return;
        }
        let texts: Vec<String> = missing.iter().map(|&i| found.lines[i].text.clone()).collect();
        std::thread::spawn(move || {
            let Some(translated) = lyrics::translate(&texts, target) else {
                return;
            };
            on_ui(move |app| {
                let found = {
                    let state = app.lyrics.borrow();
                    if state.generation != generation || state.translated_to != target {
                        return;
                    }
                    let Some(mut found) = state.found.clone() else { return };
                    for (index, text) in missing.into_iter().zip(translated) {
                        if let Some(line) = found.lines.get_mut(index) {
                            line.translation = text;
                        }
                    }
                    found
                };
                app.show_lyrics(generation, Some(found));
            });
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
        state.found = Some(found.clone());
        // As many characters as fit across the space the lyrics will have,
        // less the side padding: about 13.5 px each at the synced size, 7 at
        // the plain one.
        let room = (self.ui.get_lyrics_width() - 40.0).max(160.0);
        let (columns, height) = if found.synced {
            // Wide enough for the sung line, which is drawn larger.
            let char_px = self.ui.get_lyrics_char_px() * self.ui.get_lyrics_grow();
            ((room / char_px) as usize, 36.0)
        } else {
            ((room / 7.0) as usize, 24.0)
        };
        let rows = Rc::new(VecModel::<LyricRow>::default());
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
                        first: true,
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
            let syllable_data = |syllables: &[lyrics::Syllable]| -> ModelRc<SyllableData> {
                let data: Vec<SyllableData> = syllables
                    .iter()
                    .map(|s| SyllableData {
                        text: s.text.trim_end().into(),
                        start: s.start as i32,
                        end: s.end as i32,
                        space: s.text.ends_with(' '),
                    })
                    .collect();
                ModelRc::new(VecModel::from(data))
            };
            let (start, end) = (line.start as i32, line.end as i32);
            for (row, (text, syllables)) in wrap(line, columns).into_iter().enumerate() {
                state.rows.push((start, end, y, number));
                y += height;
                rows.push(LyricRow {
                    text: text.into(),
                    syllables: syllable_data(&syllables),
                    start,
                    end,
                    kind: 0,
                    line: number,
                    first: row == 0,
                    align: line.align as i32,
                    ..LyricRow::default()
                });
            }
            // Backing vocals, smaller, under the line. They show while it
            // is sung, so they take no room in the layout here.
            if found.synced && !line.background.is_empty() {
                let backing = lyrics::Line {
                    text: line.background_text.clone(),
                    syllables: line.background.clone(),
                    ..lyrics::Line::default()
                };
                // The smaller letters fit more to a row.
                let wider = columns * 5 / 3;
                for (text, syllables) in wrap(&backing, wider) {
                    rows.push(LyricRow {
                        text: text.into(),
                        syllables: syllable_data(&syllables),
                        start,
                        end,
                        kind: 3,
                        line: number,
                        align: line.align as i32,
                        ..LyricRow::default()
                    });
                }
            }
            // The translation, smaller and dimmer, under the line.
            if !line.translation.is_empty() {
                let translation = lyrics::Line {
                    text: line.translation.clone(),
                    ..lyrics::Line::default()
                };
                // At 0.6 of the size, and never grown.
                let wider = if found.synced { columns * 2 } else { columns };
                for text in wrap(&translation, wider).into_iter().map(|(text, _)| text) {
                    y += height * 0.6;
                    rows.push(LyricRow {
                        text: text.into(),
                        start,
                        end,
                        kind: 4,
                        line: number,
                        align: line.align as i32,
                        ..LyricRow::default()
                    });
                }
            }
        }
        state.synced = found.synced;
        // Whole lines, by line number, for the Now playing preview. It wraps
        // them itself and scrolls to `lyrics_line`.
        let texts: Vec<SharedString> = found.lines.iter().map(|l| l.text.as_str().into()).collect();
        self.ui.set_lyrics_preview(ModelRc::new(VecModel::from(texts)));
        self.ui.set_lyrics(ModelRc::from(rows.clone()));
        self.ui.set_lyrics_synced(found.synced);
        self.ui.set_lyrics_source(found.source.into());
        self.ui.set_lyrics_focus(0.0);
        self.ui.set_lyrics_line(-1);
        self.ui.set_lyrics_state(2);

        // Synced lines blur with distance from the sung one. Draw each
        // line out of focus away from the window thread, and add the
        // images to the rows as they come.
        state.layout += 1;
        let layout = state.layout;
        state.model = Some(rows.clone());
        if !found.synced {
            return;
        }
        let lines: Vec<(usize, String)> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.kind == 0)
            .map(|(index, row)| (index, row.text.to_string()))
            .collect();
        let scale = self.ui.window().scale_factor();
        let px = self.ui.get_lyrics_font() * scale;
        std::thread::spawn(move || {
            // Light and heavy blur, as standard deviations in pixels.
            let (light, heavy) = (1.2 * scale, 3.2 * scale);
            let margin = (heavy * 3.0).ceil() as u32 + 1;
            for chunk in lines.chunks(6) {
                let drawn: Vec<_> = chunk
                    .iter()
                    .filter_map(|(index, text)| {
                        let soft = blur::render(text, px, light, margin)?;
                        let softer = blur::render(text, px, heavy, margin)?;
                        Some((*index, soft, softer))
                    })
                    .collect();
                let keep_going = Arc::new(std::sync::atomic::AtomicBool::new(true));
                let flag = keep_going.clone();
                on_ui(move |app| {
                    let state = app.lyrics.borrow();
                    let Some(model) = state.model.as_ref().filter(|_| {
                        state.generation == generation && state.layout == layout
                    }) else {
                        flag.store(false, Ordering::SeqCst);
                        return;
                    };
                    for (index, soft, softer) in drawn {
                        let Some(mut row) = model.row_data(index) else { continue };
                        row.soft_pad = margin as f32 / scale;
                        row.soft_height = soft.height() as f32 / scale;
                        row.soft = Image::from_rgba8(soft);
                        row.softer = Image::from_rgba8(softer);
                        model.set_row_data(index, row);
                    }
                });
                // Stops early once the window has moved on. The answer
                // lags a chunk behind, which is fine.
                if !keep_going.load(Ordering::SeqCst) {
                    return;
                }
            }
        });
    }

    // ---------------------------------------------------------------- tick

    /// The software renderer only redraws what changed. After the window
    /// comes back from the taskbar its old pixels are gone, so ask for a
    /// full repaint by nudging the window background.
    fn repaint_after_restore(&self) {
        let window = self.ui.window();
        let minimized = window.is_minimized() || window.size().width == 0;
        if self.minimized.replace(minimized) && !minimized {
            self.ui.set_repaint(!self.ui.get_repaint());
            window.request_redraw();
        }
    }

    fn tick(self: &Rc<Self>) {
        self.repaint_after_restore();
        self.wave();
        #[cfg(windows)]
        self.attach_taskbar();
        // Shortcuts reach the window only while something holds focus.
        if !self.ui.get_typing() && !self.ui.get_hotkeys_focused() {
            self.ui.invoke_focus_hotkeys();
        }
        self.sleep_tick();
        let followed = self.audio.status.followed.swap(0, Ordering::SeqCst);
        if followed != 0 {
            self.followed(followed);
        }
        let status = &self.audio.status;
        let state = status.state.load(Ordering::SeqCst);
        // The Canvas only shows in Now playing. It holds its frame while the
        // music is paused or no one can see it, and decodes nothing meanwhile.
        let seen = !self.minimized.get() && !self.hidden.get();
        let cover_seen = seen && self.ui.get_panel() == 3;
        // The panel's cover is out of sight while the view is expanded.
        let expanded = self.ui.get_np_expanded();
        for slot in [&self.canvas, &self.wide] {
            if let Some(m) = slot.borrow().as_ref() {
                let shown = cover_seen && ((m.panel && !expanded) || (m.wide && expanded));
                m.clip.paused.store(!shown || state != audio::PLAYING, Ordering::SeqCst);
            }
        }
        let (generation, duration) = {
            let player = self.player.borrow();
            (player.generation, player.duration)
        };
        let ui = &self.ui;
        let mut time = 0u64;
        if status.generation.load(Ordering::SeqCst) == generation {
            // A saved file states its own length, which beats the guess
            // from the subtitle when there was no yt-dlp to ask.
            let stated = status.duration_ms.load(Ordering::SeqCst) as f32 / 1000.0;
            if stated > 0.0 && (stated - duration).abs() > 1.0 {
                self.player.borrow_mut().duration = stated;
                ui.set_duration_text(clock_text(stated));
            }
            let duration = if stated > 0.0 { stated } else { duration };
            let playing = state == audio::PLAYING;
            self.beat(playing && seen);
            if ui.get_playing() != playing {
                ui.set_playing(playing);
                ui.global::<Now>().set_playing(playing);
                #[cfg(windows)]
                {
                    if let Some(keys) = self.keys.borrow_mut().as_mut() {
                        keys.playing(playing);
                    }
                    taskbar::set_playing(playing);
                }
            }
            ui.set_buffering(state == audio::LOADING);
            self.track_listening(generation, playing, duration);
            if playing && self.session_saved.get().elapsed() >= SESSION_EVERY {
                self.save_session();
            }
            match state {
                audio::ENDED => {
                    status.state.store(audio::IDLE, Ordering::SeqCst);
                    self.track_ended();
                    return;
                }
                audio::FAILED => {
                    status.state.store(audio::IDLE, Ordering::SeqCst);
                    let message = status.error.lock().unwrap().clone();
                    if !self.retry_stream(&message) {
                        self.toast(message);
                    }
                }
                _ => {}
            }
            if playing && self.crossfade_check(generation, duration) {
                return;
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
            self.show_playhead(seconds);
            // Marks where the next song will start to mix in.
            let fade_from = self.mix_start(duration).map_or(1.0, |at| (at / duration).clamp(0.0, 1.0));
            ui.set_fade_from(fade_from);
            ui.set_position_text(clock_text(seconds));
            ui.set_progress(if duration > 0.0 {
                (seconds / duration).min(1.0)
            } else {
                0.0
            });
        }

        let lyrics_live = seen && matches!(ui.get_panel(), 2 | 3) && self.lyrics.borrow().synced;
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
            if cover.generation == generation && cover_seen {
                animating = true;
                if Instant::now() >= cover.due {
                    let frame = &cover.frames[cover.index];
                    // A moving cover, so it shows where Settings ask for one.
                    let (panel, wide) = self.cover_choices.get();
                    if panel == PANEL_ANIMATED {
                        ui.set_track_motion(Image::from_rgba8(frame.pixels.clone()));
                        ui.set_track_motion_on(true);
                    }
                    if wide == WIDE_ANIMATED {
                        ui.set_wide_motion(Image::from_rgba8(frame.pixels.clone()));
                        ui.set_wide_motion_on(true);
                    }
                    cover.due = Instant::now() + frame.delay;
                    cover.index = (cover.index + 1) % cover.frames.len();
                }
            }
        }

        // The song going out keeps its moving cover going until it has
        // faded, and is let go then.
        let mut fading = false;
        let faded = match self.outgoing.borrow_mut().as_mut() {
            Some(out) if Instant::now() >= out.until => true,
            Some(out) => {
                fading = true;
                let now = ui.global::<Now>();
                let length = out.until.saturating_duration_since(out.started).as_secs_f32();
                if now.get_fade_from() != out.video_id.as_str() {
                    now.set_fade_length((length * 1000.0) as i32);
                    now.set_fade_progress(0.0);
                    now.set_fade_from(out.video_id.as_str().into());
                }
                let progress = out.started.elapsed().as_secs_f32() / length.max(0.001);
                now.set_fade_progress(progress.clamp(0.0, 1.0));
                if let Some(cover) = out.cover.as_mut() {
                    if Instant::now() >= cover.due {
                        let frame = &cover.frames[cover.index];
                        ui.set_fade_motion(Image::from_rgba8(frame.pixels.clone()));
                        cover.due = Instant::now() + frame.delay;
                        cover.index = (cover.index + 1) % cover.frames.len();
                    }
                }
                false
            }
            None => false,
        };
        if faded {
            self.outgoing.borrow_mut().take();
            ui.set_fade_motion_on(false);
        }
        // No mix going, whether it ended or a skip cut it short: no pulse.
        if self.outgoing.borrow().is_none() && !ui.global::<Now>().get_fade_from().is_empty() {
            ui.global::<Now>().set_fade_from("".into());
        }

        // Tick fast only while something on screen is moving.
        // A preview's playhead moves smoothly only on the quick tick.
        let fast = animating
            || fading
            || (lyrics_live && state == audio::PLAYING)
            || self.ui.get_mix_previewing()
            || self.ui.get_mix_loading();
        if fast != self.fast.replace(fast) {
            self.timer
                .set_interval(if fast { FAST_TICK } else { SLOW_TICK });
        }
    }

    /// Starts or stops the playing bars' clock.
    fn beat(self: &Rc<Self>, on: bool) {
        if on == self.beat_timer.running() {
            return;
        }
        if !on {
            self.beat_timer.stop();
            return;
        }
        let weak = Rc::downgrade(self);
        let start = Instant::now();
        self.beat_timer.start(TimerMode::Repeated, BEAT_TICK, move || {
            if let Some(app) = weak.upgrade() {
                // Wraps after an hour, which shows as one skipped step.
                let ms = (start.elapsed().as_millis() % 3_600_000) as i32;
                app.ui.global::<Now>().set_beat(ms);
            }
        });
    }

    /// What closing the window does: hide it in the tray, or quit.
    fn close_window(self: &Rc<Self>) {
        #[cfg(windows)]
        if self.close_to_tray.get() && self.attached.get() {
            self.hidden.set(true);
            self.beat(false);
            taskbar::set_in_tray(true);
            return;
        }
        let _ = slint::quit_event_loop();
    }

    /// Hooks up the taskbar buttons and tray. The window only exists once
    /// the event loop runs, so the tick tries until it does.
    #[cfg(windows)]
    fn attach_taskbar(&self) {
        if self.attached.get() {
            return;
        }
        let attached = taskbar::attach(self.ui.window(), |action| {
            on_ui(move |app| match action {
                taskbar::Action::Previous => app.previous(),
                taskbar::Action::Toggle => app.toggle_play(),
                taskbar::Action::Next => app.next(),
                taskbar::Action::Show => app.show_window(),
                taskbar::Action::Quit => {
                    let _ = slint::quit_event_loop();
                }
            })
        });
        if attached {
            self.attached.set(true);
            taskbar::set_playing(self.ui.get_playing());
            let player = self.player.borrow();
            if let Some(item) = player.queue.get(player.index) {
                taskbar::set_song(&item.title, &credits_with_album(item).0);
            }
        }
    }

    /// Brings the window back from the tray.
    fn show_window(self: &Rc<Self>) {
        let _ = self.ui.show();
        self.hidden.set(false);
        #[cfg(windows)]
        {
            taskbar::set_in_tray(false);
            taskbar::raise(self.ui.window());
        }
    }

    fn set_close_to_tray(&self, on: bool) {
        self.close_to_tray.set(on);
        self.disk.run(move || {
            let _ = store::save("close-to-tray", if on { "1" } else { "0" });
        });
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
            // It plays, so a later failure deserves a retry again.
            self.retried.borrow_mut().take();
            *listen = Listen {
                generation,
                was_playing: true,
                last: Some(now),
                ..Listen::default()
            };
            let player = self.player.borrow();
            if let Some(item) = player.queue.get(player.index) {
                self.remember(item, 0.25);
                let (artist, album, seconds) = credits_with_album(item);
                let seconds = if duration > 0.0 { duration } else { seconds };
                let lead_artist = lead_artist(item, &artist);
                self.services.send(Event::Started(services::Track {
                    title: item.title.clone(),
                    lead_artist,
                    artist,
                    album,
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
            // Half a minute in, or half of a short song, a play counts as a
            // listen. Skips only leave the small mark from the start.
            if !listen.counted && listen.played.as_secs_f32() >= needed.min(30.0).max(5.0) {
                listen.counted = true;
                let item = {
                    let player = self.player.borrow();
                    player.queue.get(player.index).cloned()
                };
                if let Some(item) = item {
                    self.remember(&item, 0.75);
                    // And in the account's YouTube Music history, as a play
                    // in YouTube Music's own player would be.
                    if let Some(video_id) = item.video_id.clone().filter(|_| self.client.signed_in()) {
                        let client = self.client.clone();
                        self.api.run(move || {
                            if let Err(message) = client.add_to_history(&video_id) {
                                eprintln!("history: {message}");
                            }
                        });
                    }
                }
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
        self.ui
            .set_spotify_linked(!s.spotify_id.is_empty() && !s.spotify_secret.is_empty());
        self.ui
            .set_svc_spotify_cookie(s.spotify_cookie.as_str().into());
        self.ui.set_canvas_linked(!s.spotify_cookie.is_empty());
        self.ui.set_discord_on(!s.discord_id().is_empty());
        self.ui.set_discord_status(match s.discord_status.as_str() {
            "song" => 1,
            "app" => 2,
            _ => 0,
        });
        self.ui
            .set_discord_builtin(!services::DISCORD_APP.is_empty());
        self.ui.set_lastfm_builtin(
            !services::LASTFM_KEY.is_empty() && !services::LASTFM_SECRET.is_empty(),
        );
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
            // People paste "sp_dc=..." as often as the bare value.
            let cookie = self.ui.get_svc_spotify_cookie();
            let cookie = cookie.trim();
            s.spotify_cookie = cookie.strip_prefix("sp_dc=").unwrap_or(cookie).trim_end_matches(';').to_string();
        }
        self.apply_services("Saved");
        canvas::forget_token();
    }

    /// Forgets the Spotify cookie, which turns Canvas off.
    fn canvas_unlink(self: &Rc<Self>) {
        self.settings.borrow_mut().spotify_cookie.clear();
        canvas::forget_token();
        self.stop_canvas();
        self.apply_services("Canvas is off");
    }

    /// Turns Discord on or off. Discord asks for no sign in. It trusts
    /// whatever runs on the same computer, so the application ID is enough.
    fn discord_toggle(self: &Rc<Self>) {
        self.save_services();
        let on = {
            let mut s = self.settings.borrow_mut();
            let on = s.discord_id().is_empty();
            s.discord_on = if on { "1".into() } else { String::new() };
            if !on {
                s.discord_app.clear();
            }
            on
        };
        self.apply_services(if on {
            "Discord is connected. Your status shows from the next track."
        } else {
            "Discord is unlinked"
        });
    }

    /// Forgets the Spotify client ID and secret.
    fn spotify_unlink(self: &Rc<Self>) {
        {
            let mut s = self.settings.borrow_mut();
            s.spotify_id.clear();
            s.spotify_secret.clear();
        }
        self.apply_services("Spotify is unlinked");
    }

    /// Picks what Discord shows in the member list: 0 artist, 1 song, 2 app.
    fn discord_status(self: &Rc<Self>, choice: i32) {
        self.settings.borrow_mut().discord_status = match choice {
            1 => "song",
            2 => "app",
            _ => "artist",
        }
        .to_string();
        self.apply_services("");
    }

    /// Connects Last.fm, or disconnects it when it is connected. Connecting
    /// opens the approval page and waits for the user to approve there.
    fn lastfm_connect(self: &Rc<Self>) {
        self.save_services();
        if !self.settings.borrow().lastfm_session.is_empty() {
            {
                let mut s = self.settings.borrow_mut();
                s.lastfm_session.clear();
                s.lastfm_user.clear();
            }
            self.lastfm_token.borrow_mut().clear();
            self.apply_services("Last.fm is unlinked");
            return;
        }
        let (key, secret) = {
            let s = self.settings.borrow();
            let (key, secret) = s.lastfm_keys();
            (key.to_string(), secret.to_string())
        };
        if key.is_empty() || secret.is_empty() {
            self.ui
                .set_services_note("Enter your Last.fm API key and shared secret first.".into());
            return;
        }
        self.ui.set_services_note("Opening Last.fm".into());
        std::thread::spawn(move || {
            let (token, url) = match services::lastfm_begin(&key, &secret) {
                Ok(begun) => begun,
                Err(message) => {
                    on_ui(move |app| app.ui.set_services_note(message.into()));
                    return;
                }
            };
            services::open_browser(&url);
            {
                let token = token.clone();
                on_ui(move |app| {
                    *app.lastfm_token.borrow_mut() = token;
                    app.ui.set_services_note(
                        "Approve Tonearm in the browser page that opened. Tonearm connects by itself once you do."
                            .into(),
                    );
                });
            }
            // Last.fm has no callback for desktop apps, so we ask until the
            // user has approved.
            let deadline = Instant::now() + Duration::from_secs(180);
            let result = loop {
                std::thread::sleep(Duration::from_secs(3));
                match services::lastfm_finish(&key, &secret, &token) {
                    Ok(session) => break Some(session),
                    Err(_) if Instant::now() < deadline => {}
                    Err(_) => break None,
                }
            };
            on_ui(move |app| {
                if *app.lastfm_token.borrow() != token {
                    return; // A newer attempt took over, or the user disconnected.
                }
                app.lastfm_token.borrow_mut().clear();
                match result {
                    Some((session, user)) => {
                        {
                            let mut s = app.settings.borrow_mut();
                            s.lastfm_session = session;
                            s.lastfm_user = user;
                        }
                        app.apply_services("Last.fm is connected");
                    }
                    None => app.ui.set_services_note(
                        "Last.fm did not confirm in time. Press Connect to try again.".into(),
                    ),
                }
            });
        });
    }

    // ------------------------------------------------------------- sign in

    fn check_account(self: &Rc<Self>) {
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.account();
            on_ui(move |app| match result {
                Ok(Some(account)) => {
                    app.ui.set_signed_in(true);
                    app.ui.set_account_name(account.name.into());
                    app.ui.set_account_handle(account.handle.into());
                    if let Some(image) = app.thumb(&account.photo, 48, Target::Account) {
                        app.ui.set_account_art(image);
                        app.ui.set_account_has_art(true);
                    }
                    *app.account_photo.borrow_mut() = account.photo;
                    if app.ui.get_nav() == 5 {
                        app.show_profile_photo();
                    }
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
        // Signed in, yt-dlp skips the wait YouTube puts on free accounts.
        if let Some(resolver) = &self.resolver {
            resolver.set_cookie(Some(cookie.clone()));
        }
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
                            app.toast_with(note, "Update", |app| {
                                app.install_update();
                                app.toast("Downloading the update. Tonearm restarts when it is ready.");
                            });
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
        if let Some(resolver) = &self.resolver {
            resolver.set_cookie(None);
        }
        self.ui.set_signed_in(false);
        self.ui.set_account_name("".into());
        self.ui.set_account_handle("".into());
        self.ui.set_account_has_art(false);
        self.ui.set_profile_has_art(false);
        self.account_photo.borrow_mut().clear();
    }

    /// Opens the profile page over the current page, as Settings does: who
    /// is signed in, what the history on this computer says you play, and
    /// the services Tonearm reports to.
    fn show_profile(self: &Rc<Self>) {
        let (played, artists, (songs, artist_count, opened)) = {
            let taste = self.taste.borrow();
            (taste.recent(true, 12), taste.top_artist_names(5), taste.counts())
        };
        let generation = {
            let mut profile = self.profile.borrow_mut();
            profile.generation += 1;
            profile.generation
        };
        let cards: Vec<ItemData> = played
            .iter()
            .enumerate()
            .map(|(index, item)| {
                self.item_data(item, index, 156, Target::Profile { generation, index })
            })
            .collect();
        let model = Rc::new(VecModel::from(cards));
        let ui = &self.ui;
        ui.set_profile_played(ModelRc::from(model.clone()));
        ui.set_profile_artists(ModelRc::new(VecModel::from(
            artists.iter().map(|a| a.as_str().into()).collect::<Vec<SharedString>>(),
        )));
        ui.set_profile_songs(songs as i32);
        ui.set_profile_artist_count(artist_count as i32);
        ui.set_profile_opened(opened as i32);
        *self.profile.borrow_mut() = Profile { generation, played, artists, model: Some(model) };
        self.show_profile_photo();
        ui.set_nav(5);
    }

    fn show_profile_photo(self: &Rc<Self>) {
        let photo = self.account_photo.borrow().clone();
        if let Some(image) = self.thumb(&photo, 112, Target::ProfilePhoto) {
            self.ui.set_profile_art(image);
            self.ui.set_profile_has_art(true);
        }
    }

    /// A card under Played lately: plays it, as a click on Home does.
    fn profile_played(self: &Rc<Self>, index: usize) {
        let item = self.profile.borrow().played.get(index).cloned();
        if let Some(item) = item {
            self.open_item(item);
        }
    }

    /// One of the top artists: searches for them.
    fn profile_artist(self: &Rc<Self>, index: usize) {
        let name = self.profile.borrow().artists.get(index).cloned();
        if let Some(name) = name {
            self.ui.set_search_text(name.as_str().into());
            self.open(Route::Search(name, 0), true);
        }
    }
}

/// Fisher Yates with a xorshift seeded from the clock. Good enough for a
/// playlist and it saves a dependency.
/// The songs listed down a page, such as an album's tracks or an artist's
/// top songs. Rows of cards are left out.
fn songs_of(page: &Page) -> Vec<Item> {
    page.shelves
        .iter()
        .filter(|shelf| !shelf.carousel)
        .flat_map(|shelf| &shelf.items)
        .filter(|item| item.video_id.is_some())
        .cloned()
        .collect()
}

fn shuffled(mut songs: Vec<Item>) -> Vec<Item> {
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
    songs
}

/// Whether a character is drawn about twice as wide as a Latin letter, as
/// Hangul, kana and CJK ideographs are.
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F       // Hangul Jamo
        | 0x2E80..=0x303E     // CJK radicals, punctuation
        | 0x3041..=0x33FF     // kana, CJK compatibility
        | 0x3400..=0x4DBF     // CJK extension A
        | 0x4E00..=0x9FFF     // CJK ideographs
        | 0xA960..=0xA97F     // Hangul Jamo extended A
        | 0xAC00..=0xD7FF     // Hangul syllables, Jamo extended B
        | 0xF900..=0xFAFF     // CJK compatibility ideographs
        | 0xFE30..=0xFE4F     // CJK compatibility forms
        | 0xFF00..=0xFF60     // full-width forms
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF   // emoji
        | 0x20000..=0x3FFFD)  // CJK extensions B on
}

/// How many columns a piece of text takes, counting wide characters twice.
fn columns_of(text: &str) -> usize {
    text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

/// Breaks a lyric line into rows of at most `columns` columns, counting
/// wide characters twice. Breaks fall only between words, so a syllable
/// sweep never splits a word; in Japanese and Chinese, written without
/// spaces, a row may break between any two syllables.
fn wrap(line: &lyrics::Line, columns: usize) -> Vec<(String, Vec<lyrics::Syllable>)> {
    let mut rows: Vec<(String, Vec<lyrics::Syllable>)> = vec![Default::default()];
    if line.syllables.is_empty() {
        for word in line.text.split_whitespace() {
            let row = rows.last_mut().unwrap();
            if !row.0.is_empty() && columns_of(&row.0) + columns_of(word) + 1 > columns {
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
        // Korean spaces its words, so only kana and Han break anywhere.
        let spaceless = |c: char| is_wide(c) && !matches!(c as u32, 0x1100..=0x11FF | 0xA960..=0xA97F | 0xAC00..=0xD7FF);
        let ends_wide = syllable.text.chars().last().is_some_and(spaceless)
            && line.syllables.get(index + 1).is_some_and(|next| {
                next.text.chars().next().is_some_and(spaceless)
            });
        if !syllable.text.ends_with(' ') && !ends_wide && index + 1 < count {
            continue;
        }
        let length: usize = word.iter().map(|s| columns_of(&s.text)).sum();
        if !rows.last().unwrap().0.is_empty()
            && columns_of(&rows.last().unwrap().0) + length > columns
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

/// The Downloads page's key, for its mix mode and play button.
const DOWNLOADS_KEY: &str = "tonearm:downloads";

/// The Explore shelf YouTube Music draws as a ranked chart.
const CHART_TITLE: &str = "Top songs";
/// How many of the chart Explore lists. Its "more" opens the whole chart.
const CHART_ROWS: usize = 10;

/// The Explore page, with the top songs chart as a numbered list.
///
/// Signed in, Explore sends the chart as a carousel of cards; YouTube Music
/// shows it as a numbered chart, so it becomes a list here. Signed out,
/// Explore leaves it out, so we find the region's chart playlist and list
/// its top songs ourselves.
fn explore(client: &Client) -> Result<Page, String> {
    let (page, chart) = std::thread::scope(|scope| {
        let chart = scope.spawn(|| chart_shelf(client));
        (client.browse("FEmusic_explore", None), chart.join().ok().flatten())
    });
    let mut page = page?;
    let has_chart = page.shelves.iter().any(|s| s.title == CHART_TITLE);
    if let (false, Some(chart)) = (has_chart, chart) {
        // Where YouTube Music puts it: after the new releases.
        let at = page.shelves.len().min(1);
        page.shelves.insert(at, chart);
    }
    for shelf in &mut page.shelves {
        if shelf.title == CHART_TITLE && shelf.items.iter().all(|i| i.video_id.is_some()) {
            shelf.carousel = false;
            shelf.items.truncate(CHART_ROWS);
        }
    }
    Ok(page)
}

/// The region's "Top 100 Songs" chart as a shelf. The Charts page names the
/// region in its video charts, and YouTube Music's own playlists search
/// finds the song chart by that name. The id is kept for the session.
fn chart_shelf(client: &Client) -> Option<innertube::model::Shelf> {
    static CHART: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let known = CHART.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let id = match known {
        Some(id) => id,
        None => {
            let id = find_chart(client)?;
            *CHART.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
            id
        }
    };
    let playlist = client.browse(&id, None).ok()?;
    let items: Vec<Item> = playlist
        .shelves
        .into_iter()
        .flat_map(|s| s.items)
        .filter(|i| i.video_id.is_some())
        .take(CHART_ROWS)
        .collect();
    (!items.is_empty()).then(|| innertube::model::Shelf {
        title: CHART_TITLE.into(),
        carousel: false,
        items,
        more: Some((id, None)),
        ..Default::default()
    })
}

fn find_chart(client: &Client) -> Option<String> {
    const SONGS: &str = "Top 100 Songs ";
    const VIDEOS: &str = "Top 100 Music Videos ";
    let charts = client.browse("FEmusic_charts", None).ok()?;
    let mut region = None;
    for item in charts.shelves.iter().flat_map(|s| &s.items) {
        if item.title.starts_with(SONGS) {
            return item.browse_id.clone();
        }
        if let Some(name) = item.title.strip_prefix(VIDEOS) {
            region = Some(name.to_string());
        }
    }
    let region = region?;
    // The chart keeps its playlist, so once found it is kept, and the
    // unreliable search below runs only the first time.
    let key: String = region.chars().filter(char::is_ascii_alphanumeric).collect();
    let key = format!("chart-{key}");
    if let Some(id) = store::load(&key).filter(|id| id.starts_with("VL")) {
        return Some(id);
    }
    let title = format!("{SONGS}{region}");
    // The search lists the chart about half the time. Asked again, or
    // worded a little differently, it turns up.
    let queries = [title.clone(), format!("{title} YouTube Charts")];
    let id = (0..6).find_map(|attempt| {
        client
            .search_featured(&queries[attempt % 2])
            .ok()?
            .shelves
            .into_iter()
            .flat_map(|s| s.items)
            .find(|i| i.title == title)
            .and_then(|i| i.browse_id)
    })?;
    let _ = store::save(&key, &id);
    Some(id)
}

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
        "suggest" => client.suggest(arg(1)).map(|(queries, items)| {
            for query in &queries {
                println!("query {query:?}");
            }
            Page {
                shelves: vec![innertube::model::Shelf {
                    title: "Items".into(),
                    carousel: false,
                    items,
                    more: None,
                    ..Default::default()
                }],
                ..Page::default()
            }
        }),
        "browse" => client.browse(arg(1), None),
        "featured" => client.search_featured(arg(1)),
        "explore" => explore(&client),
        // The song a video in a radio stands for: --probe song <videoId>
        "song" => {
            let video = client
                .queue(Some(arg(1)), None)
                .ok()
                .and_then(|(items, _)| items.into_iter().next());
            match video {
                Some(video) => match client.song_for(&video) {
                    Some(song) => println!("{} | {} -> {} | {} {:?}", video.title, video.subtitle, song.title, song.subtitle, song.video_id),
                    None => println!("{} | {} -> no song", video.title, video.subtitle),
                },
                None => println!("no such video"),
            }
            return;
        }
        "queue" => client.queue(Some(arg(1)), None).map(|(items, _)| Page {
            shelves: vec![innertube::model::Shelf {
                title: "Queue".into(),
                carousel: false,
                items,
                more: None,
                ..Default::default()
            }],
            ..Page::default()
        }),
        "play" => {
            let Some(resolver) = resolver::default() else {
                return println!("error: no resolver");
            };
            let started = Instant::now();
            let audio = audio::Audio::new();
            audio.volume(0.0);
            // Plays from the offline cache when the track is there, and
            // fills the cache when it is not, like the app does.
            if let Some(path) = offline::local_file(arg(1)) {
                println!("playing from {}", path.display());
                audio.load(audio::Input::File(path), 1);
            } else {
                let stream = match resolver.resolve(arg(1)) {
                    Ok(stream) => stream,
                    Err(message) => return println!("error: {message}"),
                };
                println!("resolved in {:?}", started.elapsed());
                audio.load(
                    audio::Input::Http {
                        url: stream.url,
                        headers: stream.headers,
                        cache_as: Some(arg(1).to_string()),
                    },
                    1,
                );
            }
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
        "download" => {
            // `--probe download <video id>` saves a song as the app does.
            let resolver = resolver::default();
            let item = Item {
                kind: Kind::Song,
                title: arg(1).to_string(),
                video_id: Some(arg(1).to_string()),
                ..Item::default()
            };
            let http = innertube::agent(Duration::from_secs(15));
            let started = Instant::now();
            match offline::download(resolver.as_deref(), &http, &item) {
                Ok(()) => println!("downloaded in {:?}", started.elapsed()),
                Err(message) => println!("error: {message}"),
            }
            println!("downloaded {} local {:?}", offline::is_downloaded(arg(1)), offline::local_file(arg(1)));
            println!("{} songs on the Downloads page", offline::downloads().len());
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
            // As the app would: signed in when there is a session.
            resolver.set_cookie(store::load("session"));
            let started = Instant::now();
            match resolver.resolve(arg(1)) {
                Ok(stream) => {
                    println!("resolved in {:.2}s", started.elapsed().as_secs_f32());
                    println!("duration {}", stream.duration);
                    println!("via {}", resolver::last_path());
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
        // Decodes a video for three seconds: --probe clip <file.mp4>
        // A browse response as YouTube Music sends it, and its next pages:
        // --probe raw <browse id> <folder>
        "raw" => {
            let mut body = serde_json::json!({"browseId": arg(1)});
            for page in 0..6 {
                let Ok(json) = client.post("browse", body) else { break };
                let _ = std::fs::write(format!("{}/raw{page}.json", arg(2)), json.to_string());
                let token = innertube::parse::page(&json).continuation;
                let Some(token) = token else { break };
                body = serde_json::json!({"continuation": token});
            }
            return;
        }
        // Records one play in the account's YouTube Music history:
        // --probe history <video id>
        "history" => {
            match client.add_to_history(arg(1)) {
                Ok(()) => println!("recorded"),
                Err(message) => println!("failed: {message}"),
            }
            return;
        }
        // The background's flow, as frames to look at:
        // --probe wave <picture> <folder>
        "wave" => {
            let picture = image::open(arg(1)).expect("picture").to_rgba8();
            let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                picture.as_raw(),
                picture.width(),
                picture.height(),
            );
            let base = blur::backdrop(&pixels);
            for (i, t) in [0.0f32, 1.0, 2.0, 4.0, 8.0, 16.0].into_iter().enumerate() {
                let frame = blur::wave(&base, t);
                let path = format!("{}/wave{i}.png", arg(2));
                let _ = image::save_buffer(&path, frame.as_bytes(), frame.width(), frame.height(), image::ExtendedColorType::Rgba8);
            }
            return;
        }
        "clip" => {
            let frames = Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
            let seen = frames.clone();
            // Saves the first frame here when given, to look at.
            let arg_owned = arg(2).to_string();
            let clip = canvas::Clip::play_at(
                std::path::PathBuf::from(arg(1)),
                animated_art::WIDTH,
                move |p, _blur| {
                    let mut seen = seen.lock().unwrap();
                    if seen.is_empty() && !arg_owned.is_empty() {
                        let _ = image::save_buffer(&arg_owned, p.as_bytes(), p.width(), p.height(), image::ExtendedColorType::Rgba8);
                    }
                    seen.push((p.width(), p.height()));
                },
                |message| println!("failed: {message}"),
            );
            std::thread::sleep(Duration::from_secs(
                std::env::var("CLIP_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(3),
            ));
            drop(clip);
            let frames = frames.lock().unwrap();
            println!("{} frames, first {:?}", frames.len(), frames.first());
            return;
        }
        // Whether Spotify gives a Canvas: --probe canvas <title> <artist>
        "canvas" => {
            let settings = services::Settings::load();
            canvas::probe(&canvas_keys(&settings), arg(1), arg(2));
            return;
        }
        // Whether Apple Music has a moving cover: --probe motion <album> <artist>
        "motion" => {
            animated_art::probe(arg(1), arg(2));
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
                    // --probe lyrics <title> <artist> <seconds> <videoId> <language>
                    if !arg(5).is_empty() {
                        let texts: Vec<String> = found.lines.iter().map(|l| l.text.clone()).collect();
                        for line in found.lines.iter().take(3) {
                            println!("official {:?}", line.official);
                        }
                        match lyrics::translate(&texts, arg(5)) {
                            Some(done) => {
                                for (line, text) in texts.iter().zip(done).take(12) {
                                    println!("{line}\n  -> {text}");
                                }
                            }
                            None => println!("no translation"),
                        }
                    }
                }
                None => println!("no lyrics"),
            }
            return;
        }
        "thumb" => {
            let http = innertube::agent(Duration::from_secs(15));
            let started = Instant::now();
            let pixels = match arg(2).parse::<u32>() {
                Ok(width) => images::fetch_wide(&http, arg(1), width),
                Err(_) => images::fetch(&http, arg(1), 192),
            };
            match pixels {
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
            if let Some(h) = &page.header {
                println!(
                    "header {:?} label {:?} title {:?} byline {:?} {:?} meta {:?}",
                    h.kind, h.label, h.title, h.byline, h.byline_browse_id, h.meta
                );
                println!(
                    "   banner {} thumb {:?} subscribed {:?} channel {:?} liked {:?} playlist {:?}",
                    h.banner, h.thumb, h.subscribed, h.channel_id, h.liked, h.playlist_id
                );
                let endpoint = |i: &Option<Item>| {
                    i.as_ref()
                        .map(|i| (i.video_id.clone(), i.playlist_id.clone(), i.params.clone()))
                };
                println!(
                    "   shuffle {:?} radio {:?} about {} chars",
                    endpoint(&h.shuffle),
                    endpoint(&h.radio),
                    h.about.chars().count()
                );
            }
            for shelf in &page.shelves {
                println!(
                    "== {} ({} items, carousel {}) more {:?}",
                    shelf.title,
                    shelf.items.len(),
                    shelf.carousel,
                    shelf.more
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
                    if !item.artists.is_empty() || item.album.is_some() {
                        println!("      artists {:?} album {:?}", item.artists, item.album);
                    }
                }
            }
        }
        Err(message) => println!("error: {message}"),
    }
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(windows)]
    {
        shell::set_process_id();
        shell::ensure_shortcut();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--probe") {
        probe(&args[1..]);
        return Ok(());
    }
    // Already running, perhaps in the tray: that copy comes forward instead.
    #[cfg(windows)]
    let Some(instance) = single::claim() else {
        return Ok(());
    };

    update::clean_up();
    let ui = AppWindow::new()?;
    wheel::install(&ui);
    ui.set_app_version(update::VERSION.trim_start_matches('v').into());
    let client = Arc::new(Client::new());
    let session = store::load("session");
    let signed_in = session.as_deref().is_some_and(|cookie| client.set_cookie(Some(cookie)));
    let resolver: Option<Arc<dyn resolver::Resolver>> = resolver::default().map(Arc::from);
    if let (Some(resolver), Some(cookie)) = (&resolver, session.filter(|_| signed_in)) {
        resolver.set_cookie(Some(cookie));
    }

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
        resolver,
        audio: audio::Audio::new(),
        cache: RefCell::default(),
        pending: RefCell::default(),
        history: RefCell::default(),
        page: RefCell::default(),
        taste: RefCell::new(history::History::load()),
        profile: RefCell::default(),
        account_photo: RefCell::default(),
        disk: pool::Pool::new("disk", 1),
        session_saved: Cell::new(Instant::now()),
        warm: pool::Pool::new("warm", WARM_AT_ONCE),
        warming: Rc::default(),
        retried: RefCell::default(),
        playing_from: RefCell::default(),
        player: RefCell::default(),
        lyrics: RefCell::default(),
        cover: RefCell::new(None),
        canvas: RefCell::new(None),
        wide: RefCell::new(None),
        clips: Cell::new(0),
        cover_choices: Cell::new(load_cover_choices()),
        outgoing: RefCell::new(None),
        clock: Cell::new((0, Instant::now())),
        crossfade: Cell::new(load_crossfade()),
        analyses: RefCell::new(HashMap::new()),
        unanalyzed: RefCell::new(HashSet::new()),
        planned: RefCell::new(None),
        mixed_playlists: RefCell::new(library::mixed_playlists()),
        editing: RefCell::new(None),
        previewing: RefCell::new(None),
        preview_track: RefCell::new(None),
        playhead_clock: Cell::new(None),
        spinner_from: Cell::new(None),
translate: Cell::new(load_translate()),
        fade_next: RefCell::new(None),
        fade_token: Cell::new(0),
        levelling: Cell::new(load_levelling()),
        loudness: RefCell::new(HashMap::new()),
        loudness_asked: RefCell::new(HashSet::new()),
        sleep: Cell::new(Sleep::Off),
        timer: Timer::default(),
        fast: Cell::new(false),
        beat_timer: Timer::default(),
        wave_timer: Timer::default(),
        backdrop_base: RefCell::new(None),
        close_to_tray: Cell::new(load_close_to_tray()),
        hidden: Cell::new(false),
        attached: Cell::new(false),
        minimized: Cell::new(false),
        toast_timer: Timer::default(),
        toast_action: RefCell::default(),
        release: RefCell::new(None),
        suggest: RefCell::default(),
        suggest_timer: Timer::default(),
        recent_searches: RefCell::new(load_recent_searches()),
        menu: RefCell::default(),
        prompt: RefCell::default(),
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
        4 => app.open(Route::Downloads, true),
        _ => {
            app.ui.set_nav(3);
            app.show_storage();
        }
    });
    on!(on_back, |app| app.back());
    on!(on_open_profile, |app| app.show_profile());
    on!(on_profile_played_picked, |app, index| app.profile_played(index.max(0) as usize));
    on!(on_profile_played_play, |app, index| app.profile_played(index.max(0) as usize));
    on!(on_profile_artist_picked, |app, index| app.profile_artist(index.max(0) as usize));
    on!(on_search, |app, query| {
        let query = query.trim().to_string();
        if !query.is_empty() {
            app.remember_search(&query);
            app.open(Route::Search(query, 0), true);
        }
    });
    on!(on_search_edited, |app, text| app.suggest_edited(&text));
    on!(on_search_tab_picked, |app, tab| app.pick_search_tab(tab.max(0) as usize));
    on!(on_suggestion_picked, |app, index| app
        .suggestion_picked(index as usize));
    on!(on_clear_history, |app| app.clear_history());
    on!(on_item_activated, |app, id| app.activate(id as usize));
    on!(on_item_rested, |app, id| app.item_rested(id as usize));
    on!(on_item_play, |app, id| app.item_play(id as usize));
    on!(on_lyrics_width_changed, |app| app.rewrap_lyrics());
    on!(on_queue_activated, |app, index| app
        .play_index(index as usize));
    on!(on_load_more, |app| app.load_more());
    on!(on_sort_by, |app, column| app.sort_by(column));
    on!(on_recommend_add, |app, id| app.add_recommended(id as usize));
    on!(on_recommend_refresh, |app| app.refresh_recommended());
    on!(on_play_page, |app, shuffle| app.play_page(shuffle));
    on!(on_toggle_play, |app| app.toggle_play());
    on!(on_next, |app| app.next());
    on!(on_previous, |app| app.previous());
    on!(on_seek, |app, fraction| {
        let duration = app.player.borrow().duration;
        app.seek_to(fraction * duration);
    });
    on!(on_set_volume, |app, volume| app.set_volume(volume));
    {
        // Returns a value, which `on!` cannot.
        let weak = Rc::downgrade(&app);
        app.ui.on_hotkey(move |text, control, shift, alt| {
            weak.upgrade()
                .is_some_and(|app| app.hotkey(&text, control, shift, alt))
        });
    }
    on!(on_sleep_chosen, |app, choice| app.set_sleep(choice));
    on!(on_levelling_choice_changed, |app, choice| app.set_levelling(choice));
    on!(on_close_choice_changed, |app, choice| app.set_close_to_tray(choice == 0));
    on!(on_toggle_like, |app| app.toggle_like());
    on!(on_toggle_shuffle, |app| app.toggle_shuffle());
    on!(on_cycle_repeat, |app| app.cycle_repeat());
    on!(on_canvas_unlink, |app| app.canvas_unlink());
    // The drop comes from inside the dragged row's own event, and moving
    // rebuilds every row, that one too. Wait until the event is over.
    on!(on_queue_moved, |app, from, to| {
        let weak = Rc::downgrade(&app);
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(app) = weak.upgrade() {
                app.move_queued(from as usize, to as usize);
            }
        });
    });
    on!(on_prompt_done, |app, text| app.prompt_done(text.to_string()));
    on!(on_new_playlist, |app| app.ask_new_playlist(None));
    on!(on_panel_changed, |app, panel| {
        // Now playing previews the lyrics, so it needs them too.
        if panel == 2 || panel == 3 {
            app.load_lyrics();
        }
    });
    on!(on_lyrics_seek, |app, ms| app.seek_to(ms as f32 / 1000.0));
    on!(on_save_cookie, |app, text| app.save_cookie(&text));
    on!(on_sign_out, |app| app.sign_out());
    on!(on_save_services, |app| app.save_services());
    on!(on_spotify_unlink, |app| app.spotify_unlink());
    on!(on_lastfm_connect, |app| app.lastfm_connect());
    on!(on_discord_toggle, |app| app.discord_toggle());
    on!(on_discord_status_changed, |app, choice| app.discord_status(choice));
    on!(on_check_update, |app| app.check_update(false));
    on!(on_install_update, |app| app.install_update());
    on!(on_toast_clicked, |app| app.toast_clicked());
    on!(on_cache_choice_changed, |app, choice| app.set_cache_choice(choice));
    on!(on_clear_cache, |app| app.clear_cache());
    on!(on_crossfade_choice_changed, |app, choice| app.set_crossfade_choice(choice));
    on!(on_panel_cover_changed, |app, choice| {
        let wide = app.cover_choices.get().1;
        app.set_cover_choice(choice, wide);
    });
    on!(on_wide_cover_changed, |app, choice| {
        let panel = app.cover_choices.get().0;
        app.set_cover_choice(panel, choice);
    });
    on!(on_toggle_mix, |app| app.toggle_mix());
    on!(on_download_page, |app| app.download_page());
    on!(on_edit_mix, |app, id| app.edit_mix(id as usize));
    on!(on_preview_mix, |app, id| app.preview_row_mix(id as usize));
    on!(on_mix_preview, |app| app.preview_editing());
    on!(on_mix_preset_picked, |app, i| app.mix_preset_picked(i as usize));
    on!(on_mix_volume_picked, |app, i| app.mix_choice_picked(0, i as usize));
    on!(on_mix_eq_picked, |app, i| app.mix_choice_picked(1, i as usize));
    on!(on_mix_filter_picked, |app, i| app.mix_choice_picked(2, i as usize));
    on!(on_mix_effect_picked, |app, i| app.mix_choice_picked(3, i as usize));
    on!(on_mix_loop_picked, |app, i| app.mix_choice_picked(4, i as usize));
    on!(on_mix_beatmatch_toggled, |app| app.mix_beatmatch_toggled());
    on!(on_mix_key_lock_toggled, |app| app.mix_key_lock_toggled());
    on!(on_mix_save, |app| app.mix_save());
    on!(on_mix_dragged, |app, side, seconds, exact| app.mix_dragged(side, seconds as f64, exact));
    on!(on_translate_choice_changed, |app, choice| app.set_translate_choice(choice));
    // detail pages, links and the right click menu
    on!(on_show_all, |app, id| app.show_all(id as usize));
    on!(on_byline_clicked, |app| app.byline_clicked());
    on!(on_toggle_follow, |app| app.toggle_follow());
    on!(on_toggle_like_page, |app| app.toggle_like_page());
    on!(on_open_browse, |app, id| app.open_browse(&id));
    on!(on_item_menu, |app, source, id| app.item_menu(source, id));
    on!(on_menu_picked, |app, index| app.menu_picked(index as usize));
    app.show_services();
    app.ui.set_crossfade_choice(
        CROSSFADE_CHOICES
            .iter()
            .position(|&c| c == app.crossfade.get())
            .unwrap_or(0) as i32,
    );
    app.ui.set_translate_choice(app.translate.get() as i32);
    app.ui.set_levelling_choice(app.levelling.get());
    let (panel_cover, wide_cover) = app.cover_choices.get();
    app.ui.set_panel_cover(panel_cover);
    app.ui.set_wide_cover(wide_cover);
    app.ui.set_close_choice(if app.close_to_tray.get() { 0 } else { 1 });
    {
        let weak = Rc::downgrade(&app);
        app.ui.window().on_close_requested(move || {
            if let Some(app) = weak.upgrade() {
                app.close_window();
            }
            slint::CloseRequestResponse::HideWindow
        });
    }
    app.ui
        .set_suggestions(ModelRc::from(app.suggest.borrow().rows.clone()));
    app.show_recent_searches();

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
    app.ui
        .set_up_next(ModelRc::from(app.player.borrow().up_next.clone()));
    app.restore_session();
    app.repair_downloads();
    app.check_update(true);
    // Not `run`, which quits as soon as the window hides in the tray.
    #[cfg(windows)]
    instance.listen(|| on_ui(|app| app.show_window()));
    app.ui.show()?;
    let result = slint::run_event_loop_until_quit();
    let _ = app.ui.hide();
    #[cfg(windows)]
    taskbar::detach();
    // Save where we are, and wait for it behind any saves still queued.
    app.save_session();
    let (done, wait) = std::sync::mpsc::channel();
    app.disk.run(move || {
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(Duration::from_secs(3));
    APP.with(|slot| slot.borrow_mut().take());
    // Let go first, so an updated copy started now is not turned away.
    #[cfg(windows)]
    drop(instance);
    update::relaunch_if_updated();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrobbles_name_only_the_first_artist() {
        let linked = Item {
            artists: vec![
                ("Shakira".into(), Some("UC1".into())),
                ("Burna Boy".into(), Some("UC2".into())),
            ],
            ..Item::default()
        };
        assert_eq!(lead_artist(&linked, "Shakira & Burna Boy"), "Shakira");
        // A duo is one linked artist, so it stays whole.
        let duo = Item {
            artists: vec![("Simon & Garfunkel".into(), Some("UC3".into()))],
            ..Item::default()
        };
        assert_eq!(lead_artist(&duo, "Simon & Garfunkel"), "Simon & Garfunkel");
        // No links: cut at the first separator.
        let plain = Item::default();
        assert_eq!(lead_artist(&plain, "HUNTR/X, EJAE & REI AMI"), "HUNTR/X");
        assert_eq!(lead_artist(&plain, "Alex Warren"), "Alex Warren");
    }

    #[test]
    fn the_playing_song_keeps_its_place_through_moves() {
        // Moves after the playing song leave it alone.
        assert_eq!(index_after_move(2, 4, 6), 2);
        // A song moved from before it to after it pulls it up one.
        assert_eq!(index_after_move(2, 0, 5), 1);
        // A song moved from after it to before it pushes it down one.
        assert_eq!(index_after_move(2, 5, 0), 3);
        // Moving the playing song itself follows it.
        assert_eq!(index_after_move(2, 2, 7), 7);
    }

    fn songs(ids: &[&str]) -> Vec<Item> {
        ids.iter()
            .map(|id| Item {
                video_id: Some(id.to_string()),
                ..Item::default()
            })
            .collect()
    }

    fn ids(items: &[Item]) -> Vec<String> {
        items.iter().filter_map(|i| i.video_id.clone()).collect()
    }

    #[test]
    fn shuffle_starts_with_the_picked_song_and_keeps_every_song() {
        let out = shuffled_from(songs(&["a", "b", "c", "d", "e"]), 2);
        assert_eq!(out[0].video_id.as_deref(), Some("c"));
        let mut all = ids(&out);
        all.sort();
        assert_eq!(all, ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn unshuffle_restores_the_order_and_follows_the_playing_song() {
        let original = songs(&["a", "b", "c", "d"]);
        // Shuffled, with "b" removed and "x" added meanwhile.
        let queue = songs(&["c", "a", "x", "d"]);
        let (out, index) = unshuffle(original, &queue, &queue[1]);
        assert_eq!(ids(&out), ["a", "c", "d", "x"]);
        assert_eq!(index, 0);
    }

    #[test]
    fn a_play_count_is_no_artist() {
        let item = Item {
            subtitle: "2.7M plays".into(),
            artists: vec![("Wisp".into(), None)],
            ..Item::default()
        };
        assert_eq!(credits_with_album(&item).0, "Wisp");
        let plain = Item {
            subtitle: "Wisp • If Not Winter".into(),
            ..Item::default()
        };
        assert_eq!(credits_with_album(&plain).0, "Wisp");
        assert_eq!(credits_with_album(&plain).1, "If Not Winter");
    }

    #[test]
    fn a_length_is_no_artist() {
        let item = Item {
            subtitle: "Song • 2:38 • 35M plays".into(),
            artists: vec![("Don Toliver".into(), None)],
            ..Item::default()
        };
        let (artist, album, seconds) = credits_with_album(&item);
        assert_eq!(artist, "Don Toliver");
        assert_eq!(album, "");
        assert_eq!(seconds, 158.0);
    }
}
