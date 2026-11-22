//! The library database: what Tonearm works out about songs, and later the
//! mixes you make between them.
//!
//! It is one SQLite file, `library.db`, in the local data folder. It is not
//! a cache. Trimming or clearing the song cache never touches it, so a
//! song's analysis, and anything you tune by hand, outlives the audio.
//!
//! Two kinds of data live here and are kept apart. What the analyzer works
//! out can always be worked out again, so a newer analyzer replaces it.
//! What you make by hand cannot, so nothing removes it on its own.

use crate::analysis::{Analysis, ANALYZER_VERSION};
use crate::transition::Settings;
use std::collections::HashMap;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Mutex, OnceLock};

/// Schema changes, in order. Each runs once, and `user_version` counts how
/// many have. Only ever add to the end.
const MIGRATIONS: &[&str] = &[
    // 1: what the analyzer works out about a song.
    "CREATE TABLE analysis (
        video_id TEXT PRIMARY KEY,
        analyzer_version INTEGER NOT NULL,
        length REAL NOT NULL,
        bpm REAL NOT NULL,
        bpm_confidence REAL NOT NULL,
        grid_start REAL NOT NULL,
        beats BLOB NOT NULL,
        key INTEGER,
        key_confidence REAL NOT NULL,
        waveform BLOB NOT NULL,
        analyzed_at INTEGER NOT NULL
    );",
    // 2: what you choose by hand. Nothing removes these on its own.
    "CREATE TABLE transitions (
        from_id TEXT NOT NULL,
        to_id TEXT NOT NULL,
        settings TEXT NOT NULL,
        edited_at INTEGER NOT NULL,
        PRIMARY KEY (from_id, to_id)
    );
    CREATE TABLE playlist_mix (
        playlist_id TEXT PRIMARY KEY,
        edited_at INTEGER NOT NULL
    );",
];

fn db() -> std::sync::MutexGuard<'static, Option<Connection>> {
    static DB: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
    let lock = DB.get_or_init(|| {
        Mutex::new(open().map_err(|e| eprintln!("library: {e}")).ok())
    });
    lock.lock().unwrap_or_else(|e| e.into_inner())
}

fn open() -> Result<Connection, String> {
    let folder = crate::store::data_dir().ok_or("No local data folder")?;
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let mut db = Connection::open(folder.join("library.db")).map_err(|e| e.to_string())?;
    migrate(&mut db)?;
    Ok(db)
}

/// Brings the schema up to date. Each step commits on its own, so a crash
/// midway leaves a database the next start carries on from.
fn migrate(db: &mut Connection) -> Result<(), String> {
    let e = |e: rusqlite::Error| e.to_string();
    // WAL keeps a write from blocking reads, and survives a crash.
    db.pragma_update(None, "journal_mode", "WAL").map_err(e)?;
    db.pragma_update(None, "synchronous", "NORMAL").map_err(e)?;
    let done: i64 = db
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(e)?;
    if done < 0 || done as usize > MIGRATIONS.len() {
        return Err("library.db is from a newer Tonearm".into());
    }
    for (step, sql) in MIGRATIONS.iter().enumerate().skip(done as usize) {
        let tx = db.transaction().map_err(e)?;
        tx.execute_batch(sql).map_err(e)?;
        tx.pragma_update(None, "user_version", step as i64 + 1).map_err(e)?;
        tx.commit().map_err(e)?;
    }
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn floats_to_blob(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn blob_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// A song's analysis: from the current analyzer, or from an older one that
/// is still good enough to mix with, until it is redone. None when it has
/// none.
pub fn analysis(video_id: &str) -> Option<Analysis> {
    let guard = db();
    let db = guard.as_ref()?;
    db.query_row(
        "SELECT length, bpm, bpm_confidence, grid_start, beats, key, key_confidence, waveform
         FROM analysis WHERE video_id = ?1 AND analyzer_version >= ?2",
        params![video_id, crate::analysis::OLDEST_USABLE],
        |row| {
            // The beats, then a NaN, then the downbeats, so the table needs
            // no new column and older copies of Tonearm can still open it.
            let mut beats = blob_to_floats(&row.get::<_, Vec<u8>>(4)?);
            let downbeats = match beats.iter().position(|b| b.is_nan()) {
                Some(at) => {
                    let after = beats.split_off(at);
                    after[1..].to_vec()
                }
                None => Vec::new(),
            };
            Ok(Analysis {
                length: row.get(0)?,
                bpm: row.get(1)?,
                bpm_confidence: row.get(2)?,
                grid_start: row.get(3)?,
                beats,
                key: row.get::<_, Option<u8>>(5)?,
                key_confidence: row.get(6)?,
                waveform: row.get(7)?,
                downbeats,
            })
        },
    )
    .optional()
    .map_err(|e| eprintln!("library: {e}"))
    .ok()
    .flatten()
}

/// Whether a song has an analysis from the current analyzer, or a newer one.
pub fn has_analysis(video_id: &str) -> bool {
    let guard = db();
    let Some(db) = guard.as_ref() else {
        // Without the database nothing can be kept, so nothing is worth
        // analyzing either.
        return true;
    };
    db.query_row(
        "SELECT 1 FROM analysis WHERE video_id = ?1 AND analyzer_version >= ?2",
        params![video_id, ANALYZER_VERSION],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

pub fn save_analysis(video_id: &str, a: &Analysis) {
    let guard = db();
    let Some(db) = guard.as_ref() else {
        return;
    };
    let saved = db.execute(
        "INSERT OR REPLACE INTO analysis (video_id, analyzer_version, length, bpm,
            bpm_confidence, grid_start, beats, key, key_confidence, waveform, analyzed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            video_id,
            ANALYZER_VERSION,
            a.length,
            a.bpm,
            a.bpm_confidence,
            a.grid_start,
            floats_to_blob(&a.beats.iter().copied().chain([f32::NAN]).chain(a.downbeats.iter().copied()).collect::<Vec<_>>()),
            a.key,
            a.key_confidence,
            a.waveform,
            now(),
        ],
    );
    if let Err(e) = saved {
        eprintln!("library: {e}");
    }
}

/// A song's tempo and key, for a track list. No waveform or beats.
#[derive(Clone, Copy, Debug)]
pub struct Summary {
    pub bpm: f32,
    pub steady: bool,
    pub key: Option<u8>,
    pub key_confidence: f32,
}

/// The tempo and key of each of `video_ids` that has an analysis.
pub fn summaries(video_ids: &[String]) -> HashMap<String, Summary> {
    let mut found = HashMap::new();
    let guard = db();
    let Some(db) = guard.as_ref() else {
        return found;
    };
    let Ok(mut query) = db.prepare_cached(
        "SELECT bpm, bpm_confidence, length(beats) / 4, key, key_confidence
         FROM analysis WHERE video_id = ?1 AND analyzer_version >= ?2",
    ) else {
        return found;
    };
    for id in video_ids {
        let row = query
            .query_row(params![id, crate::analysis::OLDEST_USABLE], |row| {
                let (bpm, confidence, beats): (f32, f32, i64) = (row.get(0)?, row.get(1)?, row.get(2)?);
                Ok(Summary {
                    bpm,
                    // As `Analysis::steady` has it.
                    steady: bpm > 0.0 && confidence >= 0.6 && beats >= 32,
                    key: row.get(3)?,
                    key_confidence: row.get(4)?,
                })
            })
            .optional();
        if let Ok(Some(summary)) = row {
            found.insert(id.clone(), summary);
        }
    }
    found
}

/// The transition chosen by hand from one song into another.
pub fn transition(from: &str, to: &str) -> Option<Settings> {
    let guard = db();
    let text: String = guard
        .as_ref()?
        .query_row(
            "SELECT settings FROM transitions WHERE from_id = ?1 AND to_id = ?2",
            params![from, to],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten()?;
    Settings::from_json(&text)
}

/// Saves a transition chosen by hand, or with None goes back to Auto.
pub fn set_transition(from: &str, to: &str, settings: Option<Settings>) {
    let guard = db();
    let Some(db) = guard.as_ref() else {
        return;
    };
    let done = match settings {
        Some(s) => db.execute(
            "INSERT OR REPLACE INTO transitions (from_id, to_id, settings, edited_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![from, to, s.to_json(), now()],
        ),
        None => db.execute(
            "DELETE FROM transitions WHERE from_id = ?1 AND to_id = ?2",
            params![from, to],
        ),
    };
    if let Err(e) = done {
        eprintln!("library: {e}");
    }
}

/// The playlists with mix mode on.
pub fn mixed_playlists() -> std::collections::HashSet<String> {
    let guard = db();
    let Some(db) = guard.as_ref() else {
        return Default::default();
    };
    let Ok(mut query) = db.prepare("SELECT playlist_id FROM playlist_mix") else {
        return Default::default();
    };
    query
        .query_map([], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

pub fn set_playlist_mix(playlist_id: &str, on: bool) {
    let guard = db();
    let Some(db) = guard.as_ref() else {
        return;
    };
    let done = if on {
        db.execute(
            "INSERT OR REPLACE INTO playlist_mix (playlist_id, edited_at) VALUES (?1, ?2)",
            params![playlist_id, now()],
        )
    } else {
        db.execute("DELETE FROM playlist_mix WHERE playlist_id = ?1", params![playlist_id])
    };
    if let Err(e) = done {
        eprintln!("library: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_run_once_and_in_order() {
        let mut db = Connection::open_in_memory().unwrap();
        migrate(&mut db).unwrap();
        migrate(&mut db).unwrap();
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[test]
    fn floats_survive_a_blob() {
        let beats = [0.5f32, 1.0, 1.25e-3, 300.0];
        assert_eq!(blob_to_floats(&floats_to_blob(&beats)), beats);
    }
}
