//! Mix mode on playlists, and the transition editor.
//!
//! A playlist in mix mode shows each song's tempo and key, and a chip
//! between songs naming the transition from one into the next: Auto, or a
//! style chosen in the editor. Songs played from it mix into each other
//! even with crossfade off. The choices are kept per pair of songs in the
//! library database, so a transition sounds the same wherever the two meet.

use crate::analysis::Analysis;
use crate::innertube::model::Item;
#[cfg(test)]
use crate::mix::{Param, Side, Transition};
use crate::transition::{self, Preset, Settings};
use crate::{credits, library, App, AppWindow, MixSong, Place};
use slint::{Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::time::Instant;

/// A preview starts this long before the mix, so the song going out is
/// heard first, and the next song has time to open.
const PREVIEW_LEAD: f64 = 5.0;

/// The grid, and what drags snap to, between songs that do not beat match.
const FREE_STEP: f64 = 0.5;

/// A preview under way: how to find its place on the editor's picture.
pub struct PreviewTrack {
    from: String,
    to: String,
    /// Where the mix starts in the song going out, and the next song
    /// starts from, in each song's own seconds.
    out_start: f64,
    in_from: f64,
    /// The next song's speed through the mix.
    speed: f64,
    /// The picture's left and right edges, in seconds of the transition.
    left: f64,
    right: f64,
}

/// The transition being edited.
pub struct Editing {
    from: Item,
    to: Item,
    /// As shown, and as saved, to tell whether there is anything to save.
    settings: Settings,
    saved: Settings,
}

/// A key's colour on the Camelot wheel: round the hue circle by number,
/// paler for the majors.
pub fn key_color(camelot: &str) -> slint::Color {
    let color = tonearm_core::mix_editor::key_color(camelot);
    slint::Color::from_rgb_u8((color >> 16) as u8, (color >> 8) as u8, color as u8)
}

/// Sorts tempos as numbers, a rough one by its number too. Unknown is
/// empty, which sorts last.
pub fn bpm_sort_key(bpm: &str) -> String {
    bpm.trim_start_matches('~')
        .parse::<f32>()
        .map(|b| format!("{:08.2}", b))
        .unwrap_or_default()
}

/// Sorts keys round the Camelot wheel: 1A, 1B, 2A and on, so keys that mix
/// sit together.
pub fn key_sort_key(camelot: &str) -> String {
    let number: u32 = camelot.trim_end_matches(['A', 'B']).parse().unwrap_or(0);
    if number == 0 {
        return String::new();
    }
    format!("{number:02}{}", camelot.chars().last().unwrap_or('A'))
}

/// The Camelot name of a key number, as `Analysis::camelot` gives it.
fn camelot_of(key: u8) -> String {
    let (tonic, minor) = (key as usize % 12, key >= 12);
    let major = if minor { (tonic + 3) % 12 } else { tonic };
    format!("{}{}", (major * 7 + 7) % 12 + 1, if minor { 'A' } else { 'B' })
}

fn choices<T>(list: &[(T, &str)]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(list.iter().map(|(_, name)| SharedString::from(*name)).collect::<Vec<_>>()))
}

impl App {
    /// Whether the open page is a playlist in mix mode.
    fn page_mixing(&self) -> bool {
        self.ui.get_header().mixing
    }

    /// Marks a playlist's header as able to mix, and whether it does.
    pub fn show_mix_header(&self, data: &mut crate::PageHeader, playlist: bool) {
        data.can_mix = playlist;
        data.mixing = playlist && self.mixed_playlists.borrow().contains(&self.page_key());
    }

    /// The Mix button on a playlist.
    pub fn toggle_mix(self: &Rc<Self>) {
        let key = self.page_key();
        if key.is_empty() {
            return;
        }
        let on = !self.mixed_playlists.borrow().contains(&key);
        if on {
            self.mixed_playlists.borrow_mut().insert(key.clone());
        } else {
            self.mixed_playlists.borrow_mut().remove(&key);
        }
        {
            let key = key.clone();
            self.disk.run(move || library::set_playlist_mix(&key, on));
        }
        let mut header = self.ui.get_header();
        header.mixing = on;
        self.ui.set_header(header);
        // Sorted by tempo or key, which only show in mix mode: back to the
        // playlist's own order.
        if !on && self.page.borrow().sort.0 >= 3 {
            self.page.borrow_mut().sort = (0, false);
            self.ui.set_sort_column(0);
            self.ui.set_sort_descending(false);
            self.apply_sort();
        }
        if *self.playing_from.borrow() == key {
            self.forget_plan();
        }
        self.show_mix_rows();
        if on {
            self.toast("Mix mode on: songs from this playlist mix into each other");
        }
    }

    /// Fills in each song's tempo and key and the transition chips, on a
    /// playlist in mix mode. Songs not analyzed yet say why. With `queue`,
    /// songs already on this computer are queued for analysis first.
    pub fn show_mix_rows(&self) {
        self.mix_rows(true);
    }

    /// As `show_mix_rows`, for the analyzer's news: queues nothing more.
    pub fn refresh_mix_rows(&self) {
        self.mix_rows(false);
    }

    fn mix_rows(&self, queue: bool) {
        let mixing = self.page_mixing();
        let songs = self.page_songs();
        let ids: Vec<String> = songs.iter().filter_map(|(_, item)| item.video_id.clone()).collect();
        let summaries = if mixing { library::summaries(&ids) } else { Default::default() };
        // Songs already on this computer can be worked out now, so they
        // show as waiting rather than missing.
        if mixing && queue {
            for id in &ids {
                if !summaries.contains_key(id) {
                    crate::analysis::request(id);
                }
            }
        }
        let page = self.page.borrow();
        let rows = page.rows.clone();
        for (n, (id, item)) in songs.iter().enumerate() {
            let Some(Place::Row(row)) = page.places.get(*id) else {
                continue;
            };
            let Some(mut data) = rows.row_data(*row) else {
                continue;
            };
            let video_id = item.video_id.as_deref().unwrap_or("");
            let (bpm, camelot) = match summaries.get(video_id).filter(|_| mixing) {
                Some(s) => (
                    // A tempo that wanders is only roughly this.
                    match (s.bpm > 0.0, s.steady) {
                        (true, true) => format!("{:.0}", s.bpm),
                        (true, false) => format!("~{:.0}", s.bpm),
                        _ => String::new(),
                    },
                    s.key.filter(|_| s.key_confidence >= 0.4).map(camelot_of).unwrap_or_default(),
                ),
                None => (String::new(), String::new()),
            };
            // 0 known, 1 being analyzed, 2 waiting, 3 nothing to analyze yet.
            let state = if !mixing || summaries.contains_key(video_id) {
                0
            } else {
                match crate::analysis::progress(video_id) {
                    crate::analysis::Progress::Working => 1,
                    crate::analysis::Progress::Waiting => 2,
                    crate::analysis::Progress::Idle => 3,
                }
            };
            let next = songs.get(n + 1).and_then(|(_, i)| i.video_id.as_deref());
            // Out of mix mode the labels stay, so the rows can close the
            // gap under them smoothly rather than all at once.
            let mix = if !mixing {
                data.mix.to_string()
            } else {
                match next {
                    Some(next) => library::transition(video_id, next)
                        .map_or("Auto", |s| s.label())
                        .to_string(),
                    None => String::new(),
                }
            };
            if data.bpm.as_str() == bpm
                && data.camelot.as_str() == camelot
                && data.mix.as_str() == mix
                && data.analysis == state
            {
                continue;
            }
            data.key_color = key_color(&camelot);
            data.bpm = bpm.into();
            data.camelot = camelot.into();
            data.mix = mix.into();
            data.analysis = state;
            rows.set_row_data(*row, data);
        }
        // Suggestions under the playlist: tempo and key too, but no chip.
        let extras: Vec<(usize, String)> = page
            .items
            .iter()
            .zip(&page.places)
            .filter_map(|(item, place)| match place {
                Place::Extra(row) => item.video_id.clone().map(|v| (*row, v)),
                _ => None,
            })
            .collect();
        drop(page);
        if extras.is_empty() {
            return;
        }
        let extra_ids: Vec<String> = extras.iter().map(|(_, v)| v.clone()).collect();
        let summaries = if mixing { library::summaries(&extra_ids) } else { Default::default() };
        if mixing && queue {
            let missing = extra_ids.iter().filter(|v| !summaries.contains_key(*v)).cloned().collect();
            self.analyze_recommended(missing);
        }
        for (row, video_id) in extras {
            let Some(mut data) = rows.row_data(row) else {
                continue;
            };
            let (bpm, camelot) = match summaries.get(&video_id) {
                Some(s) => (
                    match (s.bpm > 0.0, s.steady) {
                        (true, true) => format!("{:.0}", s.bpm),
                        (true, false) => format!("~{:.0}", s.bpm),
                        _ => String::new(),
                    },
                    s.key.filter(|_| s.key_confidence >= 0.4).map(camelot_of).unwrap_or_default(),
                ),
                None => (String::new(), String::new()),
            };
            let state = if !mixing || summaries.contains_key(&video_id) {
                0
            } else {
                match crate::analysis::progress(&video_id) {
                    crate::analysis::Progress::Working => 1,
                    crate::analysis::Progress::Waiting => 2,
                    // Its sound is still being fetched for the analyzer.
                    crate::analysis::Progress::Idle if crate::recommend::is_fetching(&video_id) => 2,
                    crate::analysis::Progress::Idle => 3,
                }
            };
            if data.bpm.as_str() == bpm && data.camelot.as_str() == camelot && data.analysis == state {
                continue;
            }
            data.key_color = key_color(&camelot);
            data.bpm = bpm.into();
            data.camelot = camelot.into();
            data.analysis = state;
            rows.set_row_data(row, data);
        }
    }

    // ---------------------------------------------------------------- editor

    /// The chip under a song: edits the transition into the song after it.
    pub fn edit_mix(self: &Rc<Self>, id: usize) {
        let songs = self.page_songs();
        let Some(n) = songs.iter().position(|(i, _)| *i == id) else {
            return;
        };
        let (Some((_, from)), Some((_, to))) = (songs.get(n), songs.get(n + 1)) else {
            return;
        };
        let (Some(a), Some(b)) = (from.video_id.as_deref(), to.video_id.as_deref()) else {
            return;
        };
        let saved = library::transition(a, b).unwrap_or_else(|| self.auto_settings(a, b));
        *self.editing.borrow_mut() = Some(Editing {
            from: from.clone(),
            to: to.clone(),
            settings: saved,
            saved,
        });
        let ui = &self.ui;
        ui.set_mix_presets(ModelRc::new(VecModel::from(
            Preset::OFFERED.iter().map(|p| SharedString::from(p.name())).collect::<Vec<_>>(),
        )));
        ui.set_mix_volumes(choices(&transition::VOLUME_CHOICES));
        ui.set_mix_eqs(choices(&transition::EQ_CHOICES));
        ui.set_mix_filters(choices(&transition::FILTER_CHOICES));
        ui.set_mix_effects(choices(&transition::EFFECT_CHOICES));
        ui.set_mix_loops(choices(&transition::LOOP_CHOICES));
        ui.set_mix_lengths(choices(&transition::LENGTH_CHOICES));
        ui.set_mix_from(self.mix_song(id, from));
        ui.set_mix_to(self.mix_song(songs[n + 1].0, to));
        self.show_editor();
        ui.invoke_open_mix_panel();
    }

    /// What Auto picks for two songs.
    fn auto_settings(&self, from: &str, to: &str) -> Settings {
        let out = self.analysis_of(from);
        let inn = self.analysis_of(to);
        Settings::auto(out.as_deref(), inn.as_deref(), true)
    }

    fn mix_song(&self, id: usize, item: &Item) -> MixSong {
        let art = {
            let page = self.page.borrow();
            match page.places.get(id) {
                Some(Place::Row(row)) => page.rows.row_data(*row).map(|r| (r.item.art, r.item.has_art)),
                _ => None,
            }
        };
        let found = item.video_id.as_deref().and_then(|v| self.analysis_of(v));
        let summary = tonearm_core::mix_editor::song_info(found.as_deref());
        let (art, has_art) = art.unwrap_or_default();
        MixSong {
            title: item.title.as_str().into(),
            artist: credits(item).0.into(),
            art,
            has_art,
            bpm: summary.bpm.into(),
            key_color: slint::Color::from_rgb_u8(
                (summary.key_color >> 16) as u8,
                (summary.key_color >> 8) as u8,
                summary.key_color as u8,
            ),
            camelot: summary.camelot.into(),
            // `show_editor` fills it in, as it depends on the choices.
            warning: Default::default(),
        }
    }

    /// Plays the end of the song `id` into the one after it on the page,
    /// through their transition, or through `settings` from the editor. The
    /// rest of the page follows as the queue, in order.
    pub fn preview_mix(self: &Rc<Self>, id: usize, settings: Option<Settings>) {
        let songs = self.page_songs();
        let Some(n) = songs.iter().position(|(i, _)| *i == id) else {
            return;
        };
        let (Some((_, from)), Some((_, to))) = (songs.get(n), songs.get(n + 1)) else {
            return;
        };
        let (Some(a), Some(b)) = (from.video_id.clone(), to.video_id.clone()) else {
            return;
        };
        *self.previewing.borrow_mut() = settings.map(|s| (a.clone(), b.clone(), s));
        self.forget_plan();
        let chosen = self.transition_settings(&a, &b);
        let out = self.analysis_of(&a);
        let inn = self.analysis_of(&b);
        let length = out.as_ref().map_or(credits(from).1 as f64, |x| x.length as f64);
        let plan = transition::plan(chosen, out.as_deref(), length, inn.as_deref(), self.mix_length());
        let out_start = plan.out_at.unwrap_or(length - plan.length);
        let start = out_start - PREVIEW_LEAD;
        let picture = tonearm_core::mix_editor::picture_data(&plan, length, out.as_deref(), inn.as_deref(), FREE_STEP);
        *self.preview_track.borrow_mut() = Some(PreviewTrack {
            from: a.clone(),
            to: b.clone(),
            out_start,
            in_from: plan.in_from,
            speed: picture.in_speed,
            left: picture.left,
            right: picture.right,
        });
        let items = songs[n..].iter().map(|(_, item)| item.clone()).collect();
        self.start_queue(items, 0, self.page_key(), false, Some(start.max(0.0) as f32));
    }

    /// Moves the editor's playhead while a preview of its transition plays,
    /// from where the playing song is, `seconds` in.
    pub fn show_playhead(&self, seconds: f32) {
        let ui = &self.ui;
        let playing = ui.get_playing();
        let found = self.preview_time(seconds as f64);
        // The transition's own time runs at one second a second, whatever
        // speed the songs play at, so the playhead runs on a clock of its
        // own. Position reports come in steps, and the song coming in runs
        // a little fast or slow, so it leans towards them rather than
        // jumping, and only snaps on a big change, as after a seek.
        let t = found.map(|(target, _)| {
            let now = Instant::now();
            let t = match self.playhead_clock.get() {
                Some((last, at)) => {
                    let predicted = last + if playing { at.elapsed().as_secs_f64() } else { 0.0 };
                    let off = target - predicted;
                    if off.abs() > 0.75 { target } else { predicted + off * 0.12 }
                }
                None => target,
            };
            self.playhead_clock.set(Some((t, now)));
            t
        });
        if t.is_none() {
            self.playhead_clock.set(None);
        }
        // Slint moves it on between these updates, at `rate`.
        match t.zip(found) {
            Some((t, (_, (left, right)))) => {
                let at = ((t - left) / (right - left)) as f32;
                let rate = if playing { (1.0 / (right - left)) as f32 } else { 0.0 };
                ui.invoke_set_playhead(at, rate);
            }
            None if ui.get_mix_head_at() > -5.0 => ui.invoke_set_playhead(-10.0, 0.0),
            None => {}
        }
        // Stop stays stop for as long as either song of the preview plays,
        // past the picture too, as the song plays on.
        let in_preview = self.in_preview();
        let previewing = in_preview && playing;
        if ui.get_mix_previewing() != previewing {
            ui.set_mix_previewing(previewing);
        }
        // Starting up: the song or its link is still loading.
        let loading = in_preview && !playing && ui.get_buffering();
        if ui.get_mix_loading() != loading {
            ui.set_mix_loading(loading);
        }
        if loading {
            let from = self.spinner_from.get().unwrap_or_else(Instant::now);
            self.spinner_from.set(Some(from));
            // A turn a second.
            let turn = from.elapsed().as_secs_f64() % 1.0;
            ui.set_mix_spinner(spinner(turn).into());
        } else {
            self.spinner_from.set(None);
        }
    }

    /// Whether a preview of the transition being edited plays: one of its
    /// two songs is the song playing.
    fn in_preview(&self) -> bool {
        let track = self.preview_track.borrow();
        let Some(track) = track.as_ref() else {
            return false;
        };
        let editing = self.editing.borrow();
        let matches = editing.as_ref().is_some_and(|e| {
            e.from.video_id.as_deref() == Some(&track.from) && e.to.video_id.as_deref() == Some(&track.to)
        });
        let playing = self.playing_id();
        matches && (playing.as_deref() == Some(&track.from) || playing.as_deref() == Some(&track.to))
    }

    /// Where a preview of the transition being edited has got to, in
    /// seconds of the transition, with the picture's edges. None unless one
    /// of its two songs plays, and the song coming in is not past it yet.
    fn preview_time(&self, seconds: f64) -> Option<(f64, (f64, f64))> {
        let track = self.preview_track.borrow();
        let track = track.as_ref()?;
        let editing = self.editing.borrow();
        let e = editing.as_ref()?;
        if e.from.video_id.as_deref() != Some(&track.from) || e.to.video_id.as_deref() != Some(&track.to) {
            return None;
        }
        let playing = self.playing_id()?;
        let t = if playing == track.from {
            seconds - track.out_start
        } else if playing == track.to {
            (seconds - track.in_from) / track.speed
        } else {
            return None;
        };
        (t <= track.right).then_some((t, (track.left, track.right)))
    }

    /// The play button beside a transition chip: plays that transition's
    /// preview, without opening the editor.
    pub fn preview_row_mix(self: &Rc<Self>, id: usize) {
        // Plays it where it is. Only the chip opens the editor.
        self.preview_mix(id, None);
    }

    /// The editor's play button: a preview, or with one playing, pause and
    /// play.
    pub fn preview_editing(self: &Rc<Self>) {
        // Playing, it stops. Otherwise it starts over, 5 s before the switch.
        if self.ui.get_mix_previewing() {
            self.toggle_play();
            return;
        }
        self.start_editing_preview();
    }

    /// Plays the transition being edited, as it is on screen.
    fn start_editing_preview(self: &Rc<Self>) {
        let editing = self.editing.borrow().as_ref().map(|e| (e.from.clone(), e.settings));
        let Some((from, settings)) = editing else {
            return;
        };
        let id = self
            .page_songs()
            .into_iter()
            .find(|(_, item)| item.video_id.is_some() && item.video_id == from.video_id)
            .map(|(id, _)| id);
        if let Some(id) = id {
            self.preview_mix(id, Some(settings));
        }
    }

    /// About how long mixes last, in seconds.
    fn mix_length(&self) -> f64 {
        match self.crossfade.get() {
            0 => crate::MIX_MODE_SECONDS as f64,
            c => c as f64,
        }
    }

    /// A preset chip in the editor.
    pub fn mix_preset_picked(self: &Rc<Self>, index: usize) {
        let analyses = self.editor_analyses();
        self.update_mix(|settings| {
            tonearm_core::mix_editor::pick_preset(settings, index, analyses.0.as_deref(), analyses.1.as_deref())
        });
    }

    pub fn mix_choice_picked(self: &Rc<Self>, part: u8, index: usize) {
        self.update_mix(|settings| tonearm_core::mix_editor::pick_choice(settings, part as usize, index));
    }

    /// A song dragged along the picture by `seconds` of the transition:
    /// the mix moves within the song going out, or the song coming in
    /// starts from somewhere else in itself.
    /// It lands on the nearest beat, unless `exact`, as with Shift held,
    /// for fine timing.
    pub fn mix_dragged(self: &Rc<Self>, side: i32, seconds: f64, exact: bool) {
        let analyses = self.editor_analyses();
        let length = self.editing_length(&analyses.0);
        self.update_mix(|settings| {
            tonearm_core::mix_editor::drag(
                settings,
                side as usize,
                seconds,
                exact,
                analyses.0.as_deref(),
                length,
                analyses.1.as_deref(),
                self.mix_length(),
            )
        });
    }

    pub fn mix_beatmatch_toggled(self: &Rc<Self>) {
        let can = self.ui.get_mix_can_beatmatch();
        self.update_mix(|settings| Ok(tonearm_core::mix_editor::toggle_beatmatch(settings, can)));
    }

    pub fn mix_key_lock_toggled(self: &Rc<Self>) {
        let can = self.ui.get_mix_can_beatmatch();
        self.update_mix(|settings| Ok(tonearm_core::mix_editor::toggle_key_lock(settings, can)));
    }

    fn editor_analyses(&self) -> (Option<Rc<Analysis>>, Option<Rc<Analysis>>) {
        let editing = self.editing.borrow();
        let Some(e) = editing.as_ref() else { return (None, None) };
        (
            e.from.video_id.as_deref().and_then(|v| self.analysis_of(v)),
            e.to.video_id.as_deref().and_then(|v| self.analysis_of(v)),
        )
    }

    fn editing_length(&self, out: &Option<Rc<Analysis>>) -> f64 {
        let editing = self.editing.borrow();
        editing
            .as_ref()
            .map(|e| out.as_ref().map_or(credits(&e.from).1 as f64, |a| a.length as f64))
            .unwrap_or_default()
    }

    fn update_mix(self: &Rc<Self>, change: impl FnOnce(Settings) -> Result<Settings, String>) {
        let Some(settings) = self.editing.borrow().as_ref().map(|e| e.settings) else {
            return;
        };
        let Ok(settings) = change(settings) else {
            return;
        };
        if let Some(editing) = self.editing.borrow_mut().as_mut() {
            editing.settings = settings;
        }
        self.show_editor();
        // A preview playing starts over, to hear the change.
        if self.ui.get_mix_previewing() {
            self.start_editing_preview();
        }
    }

    /// Keeps the transition being edited. Auto is kept as no choice at all,
    /// so it follows the songs if their analyses change.
    pub fn mix_save(self: &Rc<Self>) {
        let Some(editing) = self.editing.borrow_mut().take() else {
            return;
        };
        let (Some(from), Some(to)) = (editing.from.video_id.clone(), editing.to.video_id.clone()) else {
            return;
        };
        let keep = (editing.settings.preset != Preset::Auto).then_some(editing.settings);
        library::set_transition(&from, &to, keep);
        self.previewing.borrow_mut().take();
        self.forget_plan();
        self.ui.invoke_close_mix_panel();
        self.show_mix_rows();
    }

    /// Puts the transition being edited on screen: its choices, what the
    /// songs allow, and the picture.
    fn show_editor(&self) {
        let editing = self.editing.borrow();
        let Some(e) = editing.as_ref() else {
            return;
        };
        let s = e.settings;
        let ui = &self.ui;
        let out = e.from.video_id.as_deref().and_then(|v| self.analysis_of(v));
        let inn = e.to.video_id.as_deref().and_then(|v| self.analysis_of(v));
        let out_length = out.as_ref().map_or(credits(&e.from).1 as f64, |a| a.length as f64);
        let snap_step = tonearm_core::mix_editor::drag_step(&s, out.as_deref(), out_length, inn.as_deref(), self.mix_length(), false);
        let data = tonearm_core::mix_editor::editor_data(s, e.saved, out.as_deref(), out_length, inn.as_deref(), self.mix_length(), snap_step);
        ui.set_mix_preset(data.preset_selected);
        ui.set_mix_length(data.choices[0].selected as i32);
        ui.set_mix_volume(data.choices[1].selected as i32);
        ui.set_mix_eq(data.choices[2].selected as i32);
        ui.set_mix_filter(data.choices[3].selected as i32);
        ui.set_mix_effect(data.choices[4].selected as i32);
        ui.set_mix_loop(data.choices[5].selected as i32);
        ui.set_mix_beatmatch(s.beatmatch);
        ui.set_mix_key_lock(s.key_lock);
        ui.set_mix_changed(data.changed);
        // Whether the songs could be beat matched at all, whatever the
        // toggle says. When not, the tempos say why and the toggle is off.
        ui.set_mix_can_beatmatch(data.can_beatmatch);
        let warning = data.why_unmatched;
        let picture = data.picture;
        ui.set_mix_span(picture.span);

        for (get, set) in [
            (AppWindow::get_mix_from as fn(&AppWindow) -> MixSong, AppWindow::set_mix_from as fn(&AppWindow, MixSong)),
            (AppWindow::get_mix_to, AppWindow::set_mix_to),
        ] {
            let mut song = get(ui);
            if song.warning.as_str() != warning {
                song.warning = warning.as_str().into();
                set(ui, song);
            }
        }
        ui.set_mix_out_wave(picture.out_wave.into());
        ui.set_mix_out_low(picture.out_low.into());
        ui.set_mix_in_wave(picture.in_wave.into());
        ui.set_mix_in_low(picture.in_low.into());
        ui.set_mix_card_from(picture.card_from);
        ui.set_mix_card_to(picture.card_to);
        ui.set_mix_volume_path(picture.volume.into());
        ui.set_mix_bass_path(picture.bass.into());
        ui.set_mix_filter_path(picture.filter.into());
        ui.set_mix_fx_path(picture.effect.into());
        ui.set_mix_bars_path(picture.bars.into());
        ui.set_mix_beats_path(picture.beats.into());

    }
}

/// The picture's size in the editor, in logical pixels: the side panel's
/// width less its padding, by the graph's height in `app.slint`. Paths are
/// drawn at this size, as Slint keeps a path's proportions.
#[cfg(test)]
pub const PICTURE_WIDTH: f64 = 340.0;
#[cfg(test)]
pub const PICTURE_HEIGHT: f64 = 230.0;

/// Draws a transition: the song going out in the top half, the one coming
/// in below, from a little before the mix to a little after.
#[cfg(test)]
struct Picture<'a> {
    plan: &'a Transition,
    out_length: f64,
    /// Seconds of the transition at the left and right edges.
    from: f64,
    to: f64,
}

/// Points along each line.
#[cfg(test)]
const STEPS: usize = 170;

#[cfg(test)]
impl<'a> Picture<'a> {
    /// The next song's speed through the mix.
    fn in_speed(&self) -> f64 {
        self.plan
            .lanes
            .iter()
            .find(|l| l.side == Side::In && l.param == Param::Speed)
            .and_then(|l| l.value(0.0))
            .unwrap_or(1.0) as f64
    }

    fn new(plan: &'a Transition, out_length: f64) -> Picture<'a> {
        let pad = (plan.length * 0.3).max(2.0);
        Picture { plan, out_length, from: -pad, to: plan.length + pad }
    }

    fn x(&self, t: f64) -> f64 {
        (t - self.from) / (self.to - self.from) * PICTURE_WIDTH
    }

    /// Where the mix starts and ends, as shares of the width, for the card
    /// drawn behind it.
    fn card(&self) -> (f32, f32) {
        ((self.x(0.0) / PICTURE_WIDTH) as f32, (self.x(self.plan.length) / PICTURE_WIDTH) as f32)
    }

    /// A song's half: its top and its height.
    fn half(side: Side) -> (f64, f64) {
        let h = PICTURE_HEIGHT / 2.0;
        if side == Side::Out { (0.0, h) } else { (h, h) }
    }

    /// A value from 0 to 1 as a height in a song's half.
    fn y(side: Side, value: f32) -> f64 {
        let (top, h) = Self::half(side);
        top + h * 0.16 + (1.0 - value.clamp(0.0, 1.0) as f64) * h * 0.68
    }

    fn line(&self, side: Side, value: impl Fn(f64) -> Option<f32>) -> String {
        let mut path = String::new();
        // Across the mix only, as on the card.
        for i in 0..=STEPS {
            let t = self.plan.length * i as f64 / STEPS as f64;
            let Some(v) = value(t) else {
                return String::new();
            };
            let op = if path.is_empty() { 'M' } else { 'L' };
            path.push_str(&format!("{op} {:.1} {:.1} ", self.x(t), Self::y(side, v)));
        }
        path
    }

    fn value(&self, side: Side, param: Param, t: f64) -> Option<f32> {
        self.plan
            .lanes
            .iter()
            .find(|l| l.side == side && l.param == param)?
            .value(t)
    }

    /// Both songs' lines for one kind of lane.
    fn lanes(&self, param: Param) -> String {
        let out = self.line(Side::Out, |t| self.value(Side::Out, param, t));
        let inn = self.line(Side::In, |t| self.value(Side::In, param, t));
        or_nothing(format!("{out}{inn}"))
    }

    /// The filter sweeps, as how much each cuts away: low on the line is
    /// open, high is closed.
    fn filters(&self) -> String {
        let span = (1000.0f32).ln();
        let out = self.line(Side::Out, |t| {
            let hz = self.value(Side::Out, Param::HighPass, t)?;
            Some(1.0 - (hz.max(20.0) / 20.0).ln() / span)
        });
        let inn = self.line(Side::In, |t| {
            let hz = self.value(Side::In, Param::LowPass, t)?;
            Some((hz.clamp(20.0, 20_000.0) / 20.0).ln() / span)
        });
        or_nothing(format!("{out}{inn}"))
    }

    /// Lines down the picture, fixed to the transition: one each `step`
    /// seconds from where the mix starts, every fourth stronger, as bars.
    /// Songs snap to these when dragged.
    fn grid(&self, step: f64) -> (String, String) {
        let (mut bars, mut beats) = (String::new(), String::new());
        if step > 0.0 {
            let first = (self.from / step).ceil() as i64;
            let last = (self.to / step).floor() as i64;
            for n in first..=last {
                let x = self.x(n as f64 * step);
                let line = format!("M {x:.1} 0 L {x:.1} {PICTURE_HEIGHT} ");
                if n.rem_euclid(4) == 0 { bars.push_str(&line) } else { beats.push_str(&line) }
            }
        }
        (or_nothing(bars), or_nothing(beats))
    }

    /// A song's loudness over the picture, mirrored about its half's
    /// middle, one column per pixel: all of it, and its bass alone. Each
    /// column takes the loudest moment it covers, so hits stay sharp.
    fn wave(&self, analysis: Option<&Analysis>, side: Side) -> (String, String) {
        let nothing = || (or_nothing(String::new()), or_nothing(String::new()));
        let Some(a) = analysis.filter(|a| a.waveform.len() >= 3 && a.length > 0.0) else {
            return nothing();
        };
        let frames = a.waveform.len() / 3;
        let rate = frames as f64 / a.length as f64;
        let speed = self.in_speed();
        let start = self.plan.out_at.unwrap_or(self.out_length - self.plan.length);
        // The song's own seconds at a moment of the transition. Before and
        // after the mix too, so a drag shows what else there is.
        let at = |t: f64| match side {
            Side::Out => Some(start + t),
            Side::In => Some(self.plan.in_from + t * speed),
        };
        // Three pictures wide, the middle one on show, so a drag brings
        // more of the song into view.
        let columns = 3 * PICTURE_WIDTH as usize;
        let step = (self.to - self.from) / PICTURE_WIDTH;
        let left = self.from - (self.to - self.from);
        let mut levels = Vec::with_capacity(columns);
        for column in 0..columns {
            let t = left + step * column as f64;
            let (Some(a0), Some(a1)) = (at(t), at(t + step)) else {
                continue;
            };
            let (f0, f1) = ((a0 * rate).floor() as isize, (a1 * rate).ceil() as isize);
            if f1 <= 0 || f0 >= frames as isize {
                continue;
            }
            let (mut all, mut low) = (0u8, 0u8);
            for f in f0.max(0) as usize..(f1.max(f0 + 1) as usize).min(frames) {
                let bands = &a.waveform[f * 3..f * 3 + 3];
                all = all.max(*bands.iter().max().unwrap_or(&0));
                low = low.max(bands[0]);
            }
            levels.push((self.x(t + step / 2.0) + PICTURE_WIDTH, all as f64 / 255.0, low as f64 / 255.0));
        }
        if levels.is_empty() {
            return nothing();
        }
        let (top, h) = Self::half(side);
        let middle = top + h / 2.0;
        let reach = h * 0.44;
        let shape = |pick: fn(&(f64, f64, f64)) -> f64| {
            let mut path = String::new();
            for (i, l) in levels.iter().enumerate() {
                let op = if i == 0 { 'M' } else { 'L' };
                path.push_str(&format!("{op} {:.1} {:.1} ", l.0, middle - pick(l) * reach));
            }
            for l in levels.iter().rev() {
                path.push_str(&format!("L {:.1} {:.1} ", l.0, middle + pick(l) * reach));
            }
            path.push('Z');
            path
        };
        // The bass, squared, so only real weight shows in it.
        (shape(|l| l.1), shape(|l| l.2 * l.2))
    }
}

/// A piece of the play button's rounded outline, `turn` of the way round
/// from the top middle, a quarter of it long. The outline sits in a 52 by
/// 40 box, one pixel in, with corners of 12.
fn spinner(turn: f64) -> String {
    let (x0, y0, w, h, r) = (1.0, 1.0, 50.0, 38.0, 12.0);
    let (straight_w, straight_h) = (w - 2.0 * r, h - 2.0 * r);
    let corner = std::f64::consts::FRAC_PI_2 * r;
    let length = 2.0 * (straight_w + straight_h) + 4.0 * corner;
    // A point `d` along the outline, clockwise from the top middle.
    let point = |d: f64| -> (f64, f64) {
        let mut d = d.rem_euclid(length);
        let arc = |cx: f64, cy: f64, from: f64, d: f64| {
            let a = from + d / r;
            (cx + r * a.cos(), cy + r * a.sin())
        };
        let half = straight_w / 2.0;
        let segments: [(f64, &dyn Fn(f64) -> (f64, f64)); 9] = [
            (half, &|d| (x0 + r + half + d, y0)),
            (corner, &|d| arc(x0 + w - r, y0 + r, -std::f64::consts::FRAC_PI_2, d)),
            (straight_h, &|d| (x0 + w, y0 + r + d)),
            (corner, &|d| arc(x0 + w - r, y0 + h - r, 0.0, d)),
            (straight_w, &|d| (x0 + w - r - d, y0 + h)),
            (corner, &|d| arc(x0 + r, y0 + h - r, std::f64::consts::FRAC_PI_2, d)),
            (straight_h, &|d| (x0, y0 + h - r - d)),
            (corner, &|d| arc(x0 + r, y0 + r, std::f64::consts::PI, d)),
            (half, &|d| (x0 + r + d, y0)),
        ];
        for (span, at) in segments {
            if d <= span {
                return at(d);
            }
            d -= span;
        }
        (x0 + r + half, y0)
    };
    let start = turn * length;
    let piece = length / 4.0;
    let mut path = String::new();
    for i in 0..=24 {
        let (x, y) = point(start + piece * i as f64 / 24.0);
        path.push_str(&format!("{} {x:.2} {y:.2} ", if i == 0 { 'M' } else { 'L' }));
    }
    path
}

/// Slint wants a path to have something in it.
#[cfg(test)]
fn or_nothing(path: String) -> String {
    if path.is_empty() { "M 0 0".into() } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spinner_stays_on_the_outline() {
        for step in 0..40 {
            let path = spinner(step as f64 / 40.0);
            let numbers: Vec<f64> = path.split_whitespace().filter_map(|w| w.parse().ok()).collect();
            for pair in numbers.chunks(2) {
                let (x, y) = (pair[0], pair[1]);
                assert!((0.9..=51.1).contains(&x) && (0.9..=39.1).contains(&y), "({x}, {y})");
                // On an edge, or on a corner's arc.
                let on_edge = (x - 1.0).abs() < 0.01 || (x - 51.0).abs() < 0.01 || (y - 1.0).abs() < 0.01 || (y - 39.0).abs() < 0.01;
                let cx = if x < 26.0 { 13.0 } else { 39.0 };
                let cy = if y < 20.0 { 13.0 } else { 27.0 };
                let on_arc = (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - 12.0).abs() < 0.01;
                assert!(on_edge || on_arc, "({x}, {y}) is off the outline");
            }
        }
    }

    #[test]
    fn tempos_and_keys_sort_in_order() {
        let mut tempos = vec!["128", "~90", "", "100"];
        tempos.sort_by_key(|b| {
            let key = bpm_sort_key(b);
            (key.is_empty(), key)
        });
        assert_eq!(tempos, ["~90", "100", "128", ""]);
        let mut keys = vec!["12A", "1B", "2A", "1A", ""];
        keys.sort_by_key(|k| {
            let key = key_sort_key(k);
            (key.is_empty(), key)
        });
        assert_eq!(keys, ["1A", "1B", "2A", "12A", ""]);
    }

    #[test]
    fn camelot_names_match_the_analysis() {
        assert_eq!(camelot_of(21), "8A");
        assert_eq!(camelot_of(0), "8B");
        assert_eq!(camelot_of(7), "9B");
    }

    #[test]
    fn a_fade_draws_inside_the_box() {
        let plan = Transition::fade(8.0);
        let picture = Picture::new(&plan, 200.0);
        let path = picture.lanes(Param::Volume);
        for number in path.split_whitespace().filter_map(|w| w.parse::<f64>().ok()) {
            assert!((0.0..=PICTURE_WIDTH).contains(&number), "{number} in {path}");
        }
        assert_eq!(picture.lanes(Param::Bass), "M 0 0");
    }
}
