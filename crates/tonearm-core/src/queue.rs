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

/// What came of asking for a song to play next or join the queue, for the
/// app to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    /// The queue was empty: the song is the queue now, and should play.
    Started,
    /// It is in the queue now (moved, if it was already further on).
    Added,
    /// It is the song playing.
    Playing,
    /// It already plays next.
    AlreadyNext,
    /// It is already somewhere in the queue.
    AlreadyQueued,
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

    /// Adds `items` at the end, as a radio does once it is found. The song
    /// playing stays where it is.
    pub fn extend(&mut self, items: impl IntoIterator<Item = T>) {
        self.items.extend(items);
    }

    /// Makes `index` the song playing. Ignored past the end.
    pub fn go(&mut self, index: usize) {
        if index < self.items.len() {
            self.index = index;
        }
    }

    /// Puts `item` right after the song playing, as the desktop's Play
    /// next: a song already in the queue moves up rather than appearing
    /// twice. `same` tells whether a queued song is this one.
    pub fn play_next(&mut self, item: T, same: impl Fn(&T) -> bool) -> Added {
        if self.items.is_empty() {
            self.set(vec![item], 0);
            return Added::Started;
        }
        if self.items.get(self.index).is_some_and(&same) {
            return Added::Playing;
        }
        if self.items.get(self.index + 1).is_some_and(&same) {
            return Added::AlreadyNext;
        }
        if let Some(at) = self.items.iter().position(&same) {
            self.items.remove(at);
            if at < self.index {
                self.index -= 1;
            }
        }
        let at = (self.index + 1).min(self.items.len());
        self.items.insert(at, item);
        Added::Added
    }

    /// Adds `item` at the end, unless it is already queued.
    pub fn add(&mut self, item: T, same: impl Fn(&T) -> bool) -> Added {
        if self.items.is_empty() {
            self.set(vec![item], 0);
            Added::Started
        } else if self.items.iter().any(same) {
            Added::AlreadyQueued
        } else {
            self.items.push(item);
            Added::Added
        }
    }

    /// Moves the song at `from` to `to`, the rest closing up behind it. The
    /// song playing stays the one playing, wherever it ends up.
    pub fn move_item(&mut self, from: usize, to: usize) {
        let len = self.items.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
        let index = self.index;
        self.index = if from == index {
            to
        } else if from < index && to >= index {
            index - 1
        } else if from > index && to <= index {
            index + 1
        } else {
            index
        };
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
    fn moving_keeps_the_song_playing() {
        // Playing 2; 4 moves up before it.
        let mut q = queue(5, 2, Repeat::Off);
        q.move_item(4, 1);
        assert_eq!(q.items(), &[0, 4, 1, 2, 3]);
        assert_eq!(q.current(), Some(&2));
        // 0 moves down past it.
        q.move_item(0, 4);
        assert_eq!(q.items(), &[4, 1, 2, 3, 0]);
        assert_eq!(q.current(), Some(&2));
        // The song playing itself moves.
        q.move_item(2, 3);
        assert_eq!(q.items(), &[4, 1, 3, 2, 0]);
        assert_eq!(q.current(), Some(&2));
        assert_eq!(q.index(), 3);
        // Out of range is ignored.
        q.move_item(9, 0);
        assert_eq!(q.items().len(), 5);
    }

    #[test]
    fn play_next_goes_after_the_song_playing() {
        let mut q = queue(5, 1, Repeat::Off);
        assert_eq!(q.play_next(9, |&s| s == 9), Added::Added);
        assert_eq!(q.items(), &[0, 1, 9, 2, 3, 4]);
        // Already queued later: moves up instead of doubling.
        assert_eq!(q.play_next(4, |&s| s == 4), Added::Added);
        assert_eq!(q.items(), &[0, 1, 4, 9, 2, 3]);
        assert_eq!(q.current(), Some(&1));
        // Queued before the song playing: the playing one keeps its place.
        assert_eq!(q.play_next(0, |&s| s == 0), Added::Added);
        assert_eq!(q.items(), &[1, 0, 4, 9, 2, 3]);
        assert_eq!(q.current(), Some(&1));
        assert_eq!(q.play_next(0, |&s| s == 0), Added::AlreadyNext);
        assert_eq!(q.play_next(1, |&s| s == 1), Added::Playing);
        let mut empty = queue(0, 0, Repeat::Off);
        assert_eq!(empty.play_next(7, |&s| s == 7), Added::Started);
        assert_eq!(empty.current(), Some(&7));
    }

    #[test]
    fn add_skips_songs_already_queued() {
        let mut q = queue(3, 0, Repeat::Off);
        assert_eq!(q.add(5, |&s| s == 5), Added::Added);
        assert_eq!(q.add(2, |&s| s == 2), Added::AlreadyQueued);
        assert_eq!(q.items(), &[0, 1, 2, 5]);
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
