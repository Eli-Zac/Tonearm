//! The sleep timer, as the desktop's: the core's `sleep` module does the
//! timing, and the queue thread pauses when it runs out.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tonearm_core::sleep::{Sleep, OPTIONS};

use crate::Tonearm;

/// The sleep timer as Now Playing shows it.
#[derive(uniffi::Record)]
pub struct SleepTimer {
    /// The choice in `sleep_options` that is running, 0 for off.
    pub choice: u32,
    /// The minutes left, or "End of song". Empty when off.
    pub text: String,
    /// Goes up each time the timer stops the music, so the app can say so
    /// once.
    pub stops: u64,
}

#[uniffi::export]
impl Tonearm {
    /// The timer's choices: Off, then the lengths, then the end of the song.
    pub fn sleep_options(&self) -> Vec<String> {
        OPTIONS.iter().map(|o| o.to_string()).collect()
    }

    /// Starts the timer from a choice in `sleep_options`, or turns it off
    /// with 0.
    pub fn set_sleep(&self, choice: u32) {
        let sleep = Sleep::from_choice(choice as usize);
        let choice = if sleep == Sleep::Off { 0 } else { choice };
        *self.sleep.lock().unwrap() = (sleep, choice);
        // A song armed to follow this one is called off by the next tick.
    }

    pub fn sleep_timer(&self) -> SleepTimer {
        let (sleep, choice) = *self.sleep.lock().unwrap();
        SleepTimer {
            choice,
            text: sleep.text(),
            stops: self.sleep_stops.load(Ordering::SeqCst),
        }
    }
}

impl Tonearm {
    /// Whether the music stops once the playing song ends.
    pub(crate) fn sleeps_after_song(&self) -> bool {
        self.sleep.lock().unwrap().0.ends_with_song()
    }

    /// Pauses once a timed timer runs out. From the queue thread's tick.
    pub(crate) fn sleep_tick(self: &Arc<Self>) {
        if self.sleep.lock().unwrap().0.is_due() {
            self.pause_here();
            self.sleep_done();
        }
    }

    /// The timer ran out. The music has stopped or is pausing.
    pub(crate) fn sleep_done(&self) {
        *self.sleep.lock().unwrap() = (Sleep::Off, 0);
        self.sleep_stops.fetch_add(1, Ordering::SeqCst);
        self.changed();
    }
}
