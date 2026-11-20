//! The queue: the songs lined up, the one playing, and what the next,
//! previous and end of a song lead to. Only the deciding lives here; the
//! app plays what it says. The rules are the desktop app's:
//!
//! - Next moves on one, and past the end starts over only with repeat on.
//! - Previous starts the song again once it is past three seconds in, or
//!   at the start of the queue with nothing to wrap to.
//! - A song that ends plays again with repeat one, or else moves on.
//!
//! The Android app uses it. The desktop app still has its own copy of
//! these rules in main.rs, to move over to this.

pub use crate::session::Repeat;

/// What a button or the end of a song leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Play the song at this place in the queue.
    Play(usize),
    /// Start the song playing again from the top.
    Restart,
    /// Nothing more to play.
    Stop,
}

/// Previous goes back a song only this early in one.
pub const RESTART_AFTER_MS: u64 = 3000;

#[derive(Clone, Debug)]
pub struct Queue<T> {
    items: Vec<T>,
    index: usize,
    pub repeat: Repeat,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Queue {
            items: Vec::new(),
            index: 0,
            repeat: Repeat::Off,
        }
    }
}

impl<T> Queue<T> {
    /// Puts `items` in the queue, at `index`.
    pub fn set(&mut self, items: Vec<T>, index: usize) {
        self.index = index.min(items.len().saturating_sub(1));
        self.items = items;
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn current(&self) -> Option<&T> {
        self.items.get(self.index)
    }

    /// Makes `index` the song playing. Ignored past the end.
    pub fn go(&mut self, index: usize) {
        if index < self.items.len() {
            self.index = index;
        }
    }

    /// The next button.
    pub fn next(&self) -> Step {
        let next = self.index + 1;
        if next < self.items.len() {
            Step::Play(next)
        } else if self.repeat != Repeat::Off && !self.items.is_empty() {
            Step::Play(0)
        } else {
            Step::Stop
        }
    }

    /// The previous button, `position_ms` into the song playing.
    pub fn previous(&self, position_ms: u64) -> Step {
        let wrap = self.index == 0 && self.repeat != Repeat::Off && self.items.len() > 1;
        if position_ms > RESTART_AFTER_MS || (self.index == 0 && !wrap) {
            Step::Restart
        } else if wrap {
            Step::Play(self.items.len() - 1)
        } else {
            Step::Play(self.index - 1)
        }
    }

    /// The song playing has run out.
    pub fn ended(&self) -> Step {
        if self.repeat == Repeat::One && self.current().is_some() {
            Step::Restart
        } else {
            self.next()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(len: usize, index: usize, repeat: Repeat) -> Queue<usize> {
        let mut queue = Queue::default();
        queue.set((0..len).collect(), index);
        queue.repeat = repeat;
        queue
    }

    #[test]
    fn next_stops_at_the_end_unless_repeating() {
        assert_eq!(queue(3, 0, Repeat::Off).next(), Step::Play(1));
        assert_eq!(queue(3, 2, Repeat::Off).next(), Step::Stop);
        assert_eq!(queue(3, 2, Repeat::All).next(), Step::Play(0));
        assert_eq!(queue(0, 0, Repeat::All).next(), Step::Stop);
    }

    #[test]
    fn previous_restarts_a_song_under_way() {
        assert_eq!(queue(3, 1, Repeat::Off).previous(500), Step::Play(0));
        assert_eq!(queue(3, 1, Repeat::Off).previous(4000), Step::Restart);
        assert_eq!(queue(3, 0, Repeat::Off).previous(500), Step::Restart);
        assert_eq!(queue(3, 0, Repeat::All).previous(500), Step::Play(2));
        assert_eq!(queue(1, 0, Repeat::All).previous(500), Step::Restart);
    }

    #[test]
    fn a_song_ending_moves_on_or_repeats() {
        assert_eq!(queue(3, 0, Repeat::Off).ended(), Step::Play(1));
        assert_eq!(queue(3, 2, Repeat::Off).ended(), Step::Stop);
        assert_eq!(queue(3, 1, Repeat::One).ended(), Step::Restart);
    }

    #[test]
    fn set_keeps_the_index_in_range() {
        let mut q = queue(0, 0, Repeat::Off);
        q.set(vec![1, 2], 5);
        assert_eq!(q.index(), 1);
        q.go(9);
        assert_eq!(q.index(), 1);
    }
}
