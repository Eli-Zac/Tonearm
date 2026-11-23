use crate::{pages, song, Song, Tonearm, TonearmError};
use std::sync::atomic::Ordering;
use tonearm_core::analysis::Analysis;
use tonearm_core::transition::{self, Preset, Settings};

/// Tempo, key and transition label shown under a song in mix mode.
#[derive(uniffi::Record, Clone, Debug)]
pub struct MixRow {
    pub from_video_id: String,
    pub to_video_id: Option<String>,
    pub bpm: String,
    pub key: String,
    pub key_color: u32,
    pub transition: String,
    pub analyzing: bool,
    /// Why there is no tempo and key yet, as the desktop's playlist says
    /// it: 0 nothing to say, 1 being analysed, 2 queued, 3 waiting for a
    /// play.
    pub analysis_state: u32,
    /// How far along the analysis is, 0 to 1, while it is being analysed.
    pub analysis_fraction: f32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MixSettings {
    pub preset: u32,
    pub volume: u32,
    pub eq: u32,
    pub filter: u32,
    pub effect: u32,
    pub looping: u32,
    pub beatmatch: bool,
    pub key_lock: bool,
    pub out_shift: f64,
    pub in_shift: f64,
    pub bars: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MixEditorChoice {
    pub title: String,
    pub options: Vec<String>,
    pub selected: u32,
    pub accent: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MixEditorSong {
    pub bpm: String,
    pub camelot: String,
    pub key_color: u32,
    pub analyzing: bool,
    /// Whether the song has been analysed. Without it the picture has no
    /// waveform to draw, so the editor shows a skeleton of one.
    pub analysed: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MixPalette {
    pub out_wave: u32,
    pub in_wave: u32,
    pub low: u32,
    pub volume: u32,
    pub bass: u32,
    pub filter: u32,
    pub effect: u32,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MixPicture {
    pub card_from: f32,
    pub card_to: f32,
    pub span: f32,
    pub out_wave: String,
    pub out_low: String,
    pub in_wave: String,
    pub in_low: String,
    pub volume: String,
    pub bass: String,
    pub filter: String,
    pub effect: String,
    pub bars: String,
    pub beats: String,
    pub left: f64,
    pub right: f64,
    pub in_speed: f64,
    pub out_start: f64,
    pub in_from: f64,
    pub palette: MixPalette,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MixEditorData {
    pub settings: MixSettings,
    pub presets: Vec<String>,
    pub preset_selected: i32,
    pub choices: Vec<MixEditorChoice>,
    pub changed: bool,
    pub can_beatmatch: bool,
    pub why_unmatched: String,
    pub beatmatch: bool,
    pub key_lock: bool,
    pub from: MixEditorSong,
    pub to: MixEditorSong,
    pub picture: MixPicture,
}

impl From<Settings> for MixSettings {
    fn from(settings: Settings) -> Self {
        use transition::*;
        Self {
            preset: Preset::OFFERED
                .iter()
                .position(|preset| *preset == settings.preset)
                .map_or(5, |index| index as u32),
            volume: index_of(&VOLUME_CHOICES, settings.volume) as u32,
            eq: index_of(&EQ_CHOICES, settings.eq) as u32,
            filter: index_of(&FILTER_CHOICES, settings.filter) as u32,
            effect: index_of(&EFFECT_CHOICES, settings.effect) as u32,
            looping: index_of(&LOOP_CHOICES, settings.looping) as u32,
            beatmatch: settings.beatmatch,
            key_lock: settings.key_lock,
            out_shift: settings.out_shift,
            in_shift: settings.in_shift,
            bars: settings.bars,
        }
    }
}

impl TryFrom<&MixSettings> for Settings {
    type Error = String;

    fn try_from(settings: &MixSettings) -> Result<Self, Self::Error> {
        use transition::*;
        let preset = match settings.preset {
            0..=4 => Preset::OFFERED[settings.preset as usize],
            5 => Preset::Custom,
            _ => return Err("Unknown transition preset".into()),
        };
        if !LENGTH_CHOICES.iter().any(|(bars, _)| *bars == settings.bars) {
            return Err("Unknown transition length".into());
        }
        if !settings.out_shift.is_finite() || !settings.in_shift.is_finite() {
            return Err("Transition timing must be finite".into());
        }
        Ok(Settings {
            preset,
            volume: choice_at(&VOLUME_CHOICES, settings.volume)?,
            eq: choice_at(&EQ_CHOICES, settings.eq)?,
            filter: choice_at(&FILTER_CHOICES, settings.filter)?,
            effect: choice_at(&EFFECT_CHOICES, settings.effect)?,
            looping: choice_at(&LOOP_CHOICES, settings.looping)?,
            beatmatch: settings.beatmatch,
            key_lock: settings.key_lock,
            out_shift: settings.out_shift,
            in_shift: settings.in_shift,
            bars: settings.bars,
        })
    }
}

fn choice_at<T: Copy>(values: &[(T, &str)], index: u32) -> Result<T, String> {
    values.get(index as usize).map(|(value, _)| *value).ok_or_else(|| "Unknown transition choice".to_string())
}

fn analysis(item: &pages::Item) -> Option<Analysis> {
    let id = item.video_id.as_deref()?;
    tonearm_core::analysis::request(id);
    tonearm_core::library::analysis(id)
}

fn settings_for(from: &pages::Item, to: &pages::Item, out: Option<&Analysis>, inn: Option<&Analysis>) -> Settings {
    from.video_id
        .as_deref()
        .zip(to.video_id.as_deref())
        .and_then(|(from, to)| tonearm_core::library::transition(from, to))
        .unwrap_or_else(|| Settings::auto(out, inn, true))
}

fn editor_data(
    from: &pages::Item,
    to: &pages::Item,
    settings: Settings,
    saved: Settings,
    out: Option<&Analysis>,
    inn: Option<&Analysis>,
) -> MixEditorData {
    let out_length = out.as_ref().map_or(crate::lyrics::seconds_of(&from.duration) as f64, |a| a.length as f64);
    let mix_length = match tonearm_core::settings::crossfade() {
        0 => transition::MIX_MODE_SECONDS,
        seconds => seconds as f64,
    };
    let step = tonearm_core::mix_editor::drag_step(&settings, out, out_length, inn, mix_length, false);
    let data = tonearm_core::mix_editor::editor_data(settings, saved, out, out_length, inn, mix_length, step);
    MixEditorData {
        settings: settings.into(),
        presets: data.presets,
        preset_selected: data.preset_selected,
        choices: data
            .choices
            .into_iter()
            .map(|choice| MixEditorChoice {
                title: choice.title,
                options: choice.options,
                selected: choice.selected as u32,
                accent: choice.accent,
            })
            .collect(),
        changed: data.changed,
        can_beatmatch: data.can_beatmatch,
        why_unmatched: data.why_unmatched,
        beatmatch: data.beatmatch,
        key_lock: data.key_lock,
        from: MixEditorSong {
            bpm: data.from.bpm,
            camelot: data.from.camelot,
            key_color: data.from.key_color,
            analyzing: from
                .video_id
                .as_deref()
                .is_some_and(|id| tonearm_core::analysis::progress(id) != tonearm_core::analysis::Progress::Idle),
            analysed: out.is_some(),
        },
        to: MixEditorSong {
            bpm: data.to.bpm,
            camelot: data.to.camelot,
            key_color: data.to.key_color,
            analyzing: to
                .video_id
                .as_deref()
                .is_some_and(|id| tonearm_core::analysis::progress(id) != tonearm_core::analysis::Progress::Idle),
            analysed: inn.is_some(),
        },
        picture: MixPicture {
            card_from: data.picture.card_from,
            card_to: data.picture.card_to,
            span: data.picture.span,
            out_wave: data.picture.out_wave,
            out_low: data.picture.out_low,
            in_wave: data.picture.in_wave,
            in_low: data.picture.in_low,
            volume: data.picture.volume,
            bass: data.picture.bass,
            filter: data.picture.filter,
            effect: data.picture.effect,
            bars: data.picture.bars,
            beats: data.picture.beats,
            left: data.picture.left,
            right: data.picture.right,
            in_speed: data.picture.in_speed,
            out_start: data.picture.out_start,
            in_from: data.picture.in_from,
            palette: MixPalette {
                out_wave: data.picture.palette.out_wave,
                in_wave: data.picture.palette.in_wave,
                low: data.picture.palette.low,
                volume: data.picture.palette.volume,
                bass: data.picture.palette.bass,
                filter: data.picture.palette.filter,
                effect: data.picture.palette.effect,
            },
        },
    }
}

fn camelot(key: u8) -> String {
    let (tonic, minor) = (key as usize % 12, key >= 12);
    let major = if minor { (tonic + 3) % 12 } else { tonic };
    format!("{}{}", (major * 7 + 7) % 12 + 1, if minor { 'A' } else { 'B' })
}

#[uniffi::export]
impl Tonearm {
    /// Whether a playlist has mix mode on.
    pub fn playlist_mix_enabled(&self, playlist_id: String) -> bool {
        tonearm_core::library::mixed_playlists().contains(&playlist_id)
    }

    /// Turns a playlist's mix mode on or off.
    pub fn set_playlist_mix(&self, playlist_id: String, on: bool) {
        tonearm_core::library::set_playlist_mix(&playlist_id, on);
        let current = self.playlist_origin.lock().unwrap().as_deref() == Some(playlist_id.as_str());
        if current {
            self.mix_mode.store(on, Ordering::SeqCst);
            tonearm_core::analysis::set_enabled(on);
            self.forget_fade();
        } else if !on {
            tonearm_core::analysis::set_enabled(self.mix_mode.load(Ordering::SeqCst));
        }
    }

    /// Starts a song from a playlist page, retaining the playlist as the
    /// queue's source even when mix mode is currently off.
    pub fn play_list_from_playlist(
        &self,
        items: Vec<pages::Item>,
        index: u32,
        playlist_id: String,
    ) -> Result<(), TonearmError> {
        let start = items.iter().take(index as usize).filter(|item| item.video_id.is_some()).count();
        let songs: Vec<Song> = items
            .iter()
            .map(pages::core_item)
            .filter_map(|item| song(&item))
            .collect();
        self.set_queue_from_playlist(songs, start as u32, Some(playlist_id))
    }

    /// Mix metadata for the ordered songs of a playlist.
    pub fn mix_rows(&self, playlist_id: String, video_ids: Vec<String>) -> Vec<MixRow> {
        if !self.playlist_mix_enabled(playlist_id) {
            return Vec::new();
        }
        tonearm_core::analysis::set_enabled(true);
        let summaries = tonearm_core::library::summaries(&video_ids);
        for id in &video_ids {
            if !summaries.contains_key(id) {
                tonearm_core::analysis::request(id);
            }
        }
        video_ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let summary = summaries.get(id);
                let next = video_ids.get(index + 1);
                let transition = next
                    .and_then(|next| tonearm_core::library::transition(id, next))
                    .map_or_else(|| if next.is_some() { "Auto" } else { "" }.to_string(), |s| s.label().to_string());
                let key = summary
                    .and_then(|s| s.key.filter(|_| s.key_confidence >= 0.4))
                    .map(camelot)
                    .unwrap_or_default();
                MixRow {
                    from_video_id: id.clone(),
                    to_video_id: next.cloned(),
                    bpm: summary
                        .filter(|s| s.bpm > 0.0)
                        .map(|s| format!("{}{:.0}", if s.steady { "" } else { "~" }, s.bpm))
                        .unwrap_or_default(),
                    key_color: tonearm_core::mix_editor::key_color(&key),
                    key,
                    transition,
                    analyzing: summary.is_none()
                        && tonearm_core::analysis::progress(id) != tonearm_core::analysis::Progress::Idle,
                    analysis_state: match summary {
                        Some(_) => 0,
                        None => match tonearm_core::analysis::progress(id) {
                            tonearm_core::analysis::Progress::Working => 1,
                            tonearm_core::analysis::Progress::Waiting => 2,
                            tonearm_core::analysis::Progress::Idle => 3,
                        },
                    },
                    analysis_fraction: tonearm_core::analysis::fraction(id),
                }
            })
            .collect()
    }

    /// Loads the transition editor's plain data for two songs.
    pub fn mix_editor(&self, from: pages::Item, to: pages::Item) -> Result<MixEditorData, TonearmError> {
        let (Some(from_id), Some(to_id)) = (from.video_id.as_deref(), to.video_id.as_deref()) else {
            return Err("Both songs need a video id".to_string().into());
        };
        tonearm_core::analysis::set_enabled(true);
        let out = analysis(&from);
        let inn = analysis(&to);
        let saved = settings_for(&from, &to, out.as_ref(), inn.as_ref());
        let initial = tonearm_core::library::transition(from_id, to_id).unwrap_or(saved);
        Ok(editor_data(&from, &to, initial, saved, out.as_ref(), inn.as_ref()))
    }

    pub fn mix_editor_view(
        &self,
        from: pages::Item,
        to: pages::Item,
        settings: MixSettings,
    ) -> Result<MixEditorData, TonearmError> {
        let (Some(from_id), Some(to_id)) = (from.video_id.as_deref(), to.video_id.as_deref()) else {
            return Err("Both songs need a video id".to_string().into());
        };
        tonearm_core::analysis::set_enabled(true);
        let out = analysis(&from);
        let inn = analysis(&to);
        let saved = settings_for(&from, &to, out.as_ref(), inn.as_ref());
        let current = Settings::try_from(&settings)?;
        let _ = (from_id, to_id);
        Ok(editor_data(&from, &to, current, saved, out.as_ref(), inn.as_ref()))
    }

    pub fn mix_editor_preset(
        &self,
        from: pages::Item,
        to: pages::Item,
        settings: MixSettings,
        index: u32,
    ) -> Result<MixSettings, TonearmError> {
        let (Some(_), Some(_)) = (from.video_id.as_deref(), to.video_id.as_deref()) else {
            return Err("Both songs need a video id".to_string().into());
        };
        tonearm_core::analysis::set_enabled(true);
        let out = analysis(&from);
        let inn = analysis(&to);
        let current = Settings::try_from(&settings)?;
        Ok(tonearm_core::mix_editor::pick_preset(current, index as usize, out.as_ref(), inn.as_ref())?.into())
    }

    pub fn mix_editor_choice(&self, settings: MixSettings, part: u32, index: u32) -> Result<MixSettings, TonearmError> {
        Ok(tonearm_core::mix_editor::pick_choice(Settings::try_from(&settings)?, part as usize, index as usize)?.into())
    }

    pub fn mix_editor_drag(
        &self,
        from: pages::Item,
        to: pages::Item,
        settings: MixSettings,
        side: u32,
        seconds: f64,
        exact: bool,
    ) -> Result<MixSettings, TonearmError> {
        let (Some(_), Some(_)) = (from.video_id.as_deref(), to.video_id.as_deref()) else {
            return Err("Both songs need a video id".to_string().into());
        };
        if !seconds.is_finite() {
            return Err("Transition timing must be finite".to_string().into());
        }
        tonearm_core::analysis::set_enabled(true);
        let out = analysis(&from);
        let inn = analysis(&to);
        let out_length = out.as_ref().map_or(crate::lyrics::seconds_of(&from.duration) as f64, |a| a.length as f64);
        let mix_length = match tonearm_core::settings::crossfade() {
            0 => transition::MIX_MODE_SECONDS,
            seconds => seconds as f64,
        };
        Ok(tonearm_core::mix_editor::drag(
            Settings::try_from(&settings)?,
            side as usize,
            seconds,
            exact,
            out.as_ref(),
            out_length,
            inn.as_ref(),
            mix_length,
        )?
        .into())
    }

    pub fn mix_editor_beatmatch(&self, settings: MixSettings, can_beatmatch: bool) -> Result<MixSettings, TonearmError> {
        Ok(tonearm_core::mix_editor::toggle_beatmatch(Settings::try_from(&settings)?, can_beatmatch).into())
    }

    pub fn mix_editor_key_lock(&self, settings: MixSettings, can_beatmatch: bool) -> Result<MixSettings, TonearmError> {
        Ok(tonearm_core::mix_editor::toggle_key_lock(Settings::try_from(&settings)?, can_beatmatch).into())
    }

    pub fn set_transition(
        &self,
        from: pages::Item,
        to: pages::Item,
        settings: MixSettings,
    ) -> Result<(), TonearmError> {
        let (Some(from_id), Some(to_id)) = (from.video_id.as_deref(), to.video_id.as_deref()) else {
            return Err("Both songs need a video id".to_string().into());
        };
        let settings = Settings::try_from(&settings)?;
        tonearm_core::library::set_transition(from_id, to_id, (settings.preset != Preset::Auto).then_some(settings));
        self.preview_mix.lock().unwrap().take();
        self.forget_fade();
        Ok(())
    }

    /// Plays two songs from just before their edited transition.
    pub fn preview_transition(
        &self,
        from: pages::Item,
        to: pages::Item,
        settings: MixSettings,
    ) -> Result<(), TonearmError> {
        let current = Settings::try_from(&settings)?;
        let (Some(from_id), Some(to_id)) = (from.video_id.clone(), to.video_id.clone()) else {
            return Err("Both songs need a video id".to_string().into());
        };
        let out = analysis(&from);
        let inn = analysis(&to);
        let duration = out.as_ref().map_or(crate::lyrics::seconds_of(&from.duration) as f64, |a| a.length as f64);
        let mix_length = match tonearm_core::settings::crossfade() {
            0 => transition::MIX_MODE_SECONDS,
            seconds => seconds as f64,
        };
        let plan = transition::plan(current, out.as_ref(), duration, inn.as_ref(), mix_length);
        let start = (plan.out_at.unwrap_or(duration - plan.length) - 5.0).max(0.0);
        let picture = tonearm_core::mix_editor::picture_data(
            &plan,
            duration,
            out.as_ref(),
            inn.as_ref(),
            tonearm_core::mix_editor::drag_step(&current, out.as_ref(), duration, inn.as_ref(), mix_length, false),
        );
        let songs = [from, to]
            .into_iter()
            .map(|item| pages::core_item(&item))
            .filter_map(|item| song(&item))
            .collect::<Vec<_>>();
        if songs.len() != 2 {
            return Err("Both songs need a video id".to_string().into());
        }
        *self.resume_at.lock().unwrap() = Some((from_id.clone(), (start * 1000.0) as u64));
        self.set_queue_from_playlist(songs, 0, None)?;
        *self.preview_mix.lock().unwrap() = Some(crate::PreviewMix {
            from: from_id,
            to: to_id,
            settings: current,
            out_start: plan.out_at.unwrap_or(duration - plan.length),
            in_from: plan.in_from,
            speed: picture.in_speed,
            left: picture.left,
            right: picture.right,
        });
        self.mix_mode.store(true, Ordering::SeqCst);
        tonearm_core::analysis::set_enabled(true);
        Ok(())
    }

    /// Where the active preview is in the editor's picture.
    pub fn mix_preview_position(&self) -> Option<f32> {
        let preview = self.preview_mix.lock().unwrap();
        let preview = preview.as_ref()?;
        let song = self.now_playing()?;
        // What is heard, not what the mixer has made: while casting, the
        // device plays the stream a moment behind it.
        let status = &self.audio.status;
        let position = status
            .position_ms
            .load(Ordering::SeqCst)
            .saturating_sub(status.lag_ms.load(Ordering::SeqCst)) as f64
            / 1000.0;
        let at = if song.video_id == preview.from {
            position - preview.out_start
        } else if song.video_id == preview.to {
            (position - preview.in_from) / preview.speed
        } else {
            return None;
        };
        (at >= preview.left && at <= preview.right)
            .then_some(((at - preview.left) / (preview.right - preview.left)) as f32)
    }

    /// Whether a preview plays: one of its two songs is the song playing,
    /// and it has not ended or failed.
    pub fn mix_preview_active(&self) -> bool {
        let preview = self.preview_mix.lock().unwrap();
        let Some(preview) = preview.as_ref() else {
            return false;
        };
        let Some(song) = self.now_playing() else {
            return false;
        };
        if song.video_id != preview.from && song.video_id != preview.to {
            return false;
        }
        !matches!(
            self.audio.status.state.load(Ordering::SeqCst),
            tonearm_core::audio::ENDED | tonearm_core::audio::FAILED
        )
    }
}
