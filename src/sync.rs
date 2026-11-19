//! Two-way playlist sync: what to change on each side, worked out from
//! both playlists as they are now and as they were after the last sync.
//!
//! This is only the deciding, with no network in it, so every rule can be
//! tested. Songs are named by keys both sides share: the YouTube Music
//! video id, which a Spotify song has once it is matched. A Spotify song
//! with no match keeps a key of its own, which the other side never has.
//!
//! The rules, as agreed before building it:
//!  - Each side's own list after the last sync is kept. A song missing now
//!    that was there then was removed; one there now that was not, added.
//!    Without that, a song on one side only could be either.
//!  - The first sync only adds: each side gets what it lacks.
//!  - A song the other side cannot have (a Spotify song not on YouTube
//!    Music) is never sent across, and its absence there is never read as
//!    a removal, because it was never there to remove.
//!  - Removing more than a few songs, or a large share of a playlist, is
//!    held back for the user to agree to first.
//!  - A side that cannot be written, such as a Spotify-made playlist, only
//!    gives: nothing is added to it or removed from it.
//!  - New songs go at the end, in the order they have on their own side.


use std::collections::HashSet;

/// One playlist of a pair, as it is now.
pub struct Side<'a> {
    /// Its songs' keys, in order.
    pub songs: &'a [String],
    /// Whether Tonearm may change it.
    pub writable: bool,
    /// Keys only this side can have: songs the other side has no match for.
    pub only_here: &'a HashSet<String>,
}

/// What a sync would do.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub add_to_a: Vec<String>,
    pub add_to_b: Vec<String>,
    pub remove_from_a: Vec<String>,
    pub remove_from_b: Vec<String>,
    /// Set when the removals are big enough to ask about first. The adds
    /// can still go ahead.
    pub hold_removals: bool,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.add_to_a.is_empty()
            && self.add_to_b.is_empty()
            && self.remove_from_a.is_empty()
            && self.remove_from_b.is_empty()
    }
}

/// More removals than this from one side are held, however long it is.
const HOLD_COUNT: usize = 10;
/// Removing more than this share of a side is held, once it is a few
/// songs: taking one song out of a short playlist needs no asking.
const HOLD_SHARE: f64 = 0.25;
const HOLD_SHARE_FROM: usize = 3;

fn without(list: &[String], drop: &HashSet<&String>) -> Vec<String> {
    list.iter().filter(|k| !drop.contains(k)).cloned().collect()
}

/// The changes that bring `a` and `b` together. `base` is each side's list
/// after the last sync, or None before the first.
pub fn plan(a: &Side, b: &Side, base: Option<(&[String], &[String])>) -> Plan {
    let in_a: HashSet<&String> = a.songs.iter().collect();
    let in_b: HashSet<&String> = b.songs.iter().collect();
    // What each side gained and lost since last time. Before the first
    // sync, everything counts as gained and nothing as lost.
    let (gained_a, gained_b, lost_a, lost_b): (Vec<&String>, Vec<&String>, Vec<&String>, Vec<&String>) = match base {
        None => (a.songs.iter().collect(), b.songs.iter().collect(), Vec::new(), Vec::new()),
        Some((was_a, was_b)) => {
            let was_a_set: HashSet<&String> = was_a.iter().collect();
            let was_b_set: HashSet<&String> = was_b.iter().collect();
            (
                a.songs.iter().filter(|k| !was_a_set.contains(k)).collect(),
                b.songs.iter().filter(|k| !was_b_set.contains(k)).collect(),
                was_a.iter().filter(|k| !in_a.contains(k)).collect(),
                was_b.iter().filter(|k| !in_b.contains(k)).collect(),
            )
        }
    };
    let mut plan = Plan::default();
    let mut seen = HashSet::new();
    if b.writable {
        plan.add_to_b = gained_a
            .iter()
            .filter(|k| !in_b.contains(*k) && !a.only_here.contains(**k) && seen.insert(**k))
            .map(|k| (*k).clone())
            .collect();
        // Gone from A, so it goes from B too, unless B itself added it again
        // since, which counts as wanting it.
        plan.remove_from_b = lost_a
            .iter()
            .filter(|k| in_b.contains(*k) && !gained_b.contains(k))
            .map(|k| (*k).clone())
            .collect();
    }
    seen.clear();
    if a.writable {
        plan.add_to_a = gained_b
            .iter()
            .filter(|k| !in_a.contains(*k) && !b.only_here.contains(**k) && seen.insert(**k))
            .map(|k| (*k).clone())
            .collect();
        plan.remove_from_a = lost_b
            .iter()
            .filter(|k| in_a.contains(*k) && !gained_a.contains(k))
            .map(|k| (*k).clone())
            .collect();
    }
    let too_many = |removed: usize, of: usize| {
        removed > HOLD_COUNT || (removed >= HOLD_SHARE_FROM && removed as f64 > of as f64 * HOLD_SHARE)
    };
    plan.hold_removals = too_many(plan.remove_from_a.len(), a.songs.len()) || too_many(plan.remove_from_b.len(), b.songs.len());
    plan
}

/// Each side's list once a plan is carried out, to keep as the next
/// sync's `base`. Held removals stay pending: the side they were removed
/// from keeps them in its base, so the next sync sees them as removed
/// again, until the user agrees (`agreed`) or keeps them (`kept`).
pub fn after(a: &[String], b: &[String], plan: &Plan) -> (Vec<String>, Vec<String>) {
    let mut next_a: Vec<String>;
    let mut next_b: Vec<String>;
    if plan.hold_removals {
        next_a = a.to_vec();
        next_b = b.to_vec();
        // Removed from B, waiting to go from A: A's base still has them.
        next_b.extend(plan.remove_from_a.iter().filter(|k| !b.contains(k)).cloned());
        next_a.extend(plan.remove_from_b.iter().filter(|k| !a.contains(k)).cloned());
    } else {
        next_a = without(a, &plan.remove_from_a.iter().collect());
        next_b = without(b, &plan.remove_from_b.iter().collect());
    }
    next_a.extend(plan.add_to_a.iter().cloned());
    next_b.extend(plan.add_to_b.iter().cloned());
    (next_a, next_b)
}

/// The bases once held removals are kept rather than made: each side that
/// lost them forgets it had them, so they are no longer removals.
pub fn kept(base: (&[String], &[String]), from_a: &[String], from_b: &[String]) -> (Vec<String>, Vec<String>) {
    // A song held for removal from A was lost by B, and the other way.
    let lost_by_b: HashSet<&String> = from_a.iter().collect();
    let lost_by_a: HashSet<&String> = from_b.iter().collect();
    (without(base.0, &lost_by_a), without(base.1, &lost_by_b))
}

/// The bases once held removals are made: gone from both.
pub fn agreed(base: (&[String], &[String]), from_a: &[String], from_b: &[String]) -> (Vec<String>, Vec<String>) {
    let gone: HashSet<&String> = from_a.iter().chain(from_b).collect();
    (without(base.0, &gone), without(base.1, &gone))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn side<'a>(songs: &'a [String], only_here: &'a HashSet<String>) -> Side<'a> {
        Side { songs, writable: true, only_here }
    }

    #[test]
    fn the_first_sync_only_adds() {
        let none = HashSet::new();
        let (a, b) = (keys(&["1", "2", "3"]), keys(&["3", "4"]));
        let p = plan(&side(&a, &none), &side(&b, &none), None);
        assert_eq!(p.add_to_b, keys(&["1", "2"]));
        assert_eq!(p.add_to_a, keys(&["4"]));
        assert!(p.remove_from_a.is_empty() && p.remove_from_b.is_empty());
    }

    #[test]
    fn a_song_added_on_one_side_goes_to_the_other() {
        let none = HashSet::new();
        let base = (keys(&["1", "2"]), keys(&["1", "2"]));
        let (a, b) = (keys(&["1", "2", "5"]), keys(&["1", "2"]));
        let p = plan(&side(&a, &none), &side(&b, &none), Some((&base.0, &base.1)));
        assert_eq!(p, Plan { add_to_b: keys(&["5"]), ..Plan::default() });
    }

    #[test]
    fn a_song_removed_on_one_side_goes_from_the_other() {
        let none = HashSet::new();
        let base = (keys(&["1", "2", "3", "4", "5"]), keys(&["1", "2", "3", "4", "5"]));
        let (a, b) = (keys(&["1", "2", "3", "4"]), keys(&["1", "2", "3", "4", "5"]));
        let p = plan(&side(&a, &none), &side(&b, &none), Some((&base.0, &base.1)));
        assert_eq!(p.remove_from_b, keys(&["5"]));
        assert!(!p.hold_removals);
    }

    #[test]
    fn a_song_the_other_side_cannot_have_is_neither_sent_nor_removed() {
        // "sp:x" is a Spotify song YouTube Music does not have.
        let only_spotify: HashSet<String> = ["sp:x".to_string()].into();
        let none = HashSet::new();
        let (a, b) = (keys(&["1", "sp:x"]), keys(&["1"]));
        let first = plan(&side(&a, &only_spotify), &side(&b, &none), None);
        assert!(first.is_empty(), "{first:?}");
        // Next time, its absence on YouTube Music is not a removal.
        let base = after(&a, &b, &first);
        let again = plan(&side(&a, &only_spotify), &side(&b, &none), Some((&base.0, &base.1)));
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn many_removals_are_held_for_the_user() {
        let none = HashSet::new();
        let all: Vec<String> = (0..40).map(|n| n.to_string()).collect();
        let kept: Vec<String> = all[..20].to_vec();
        let p = plan(&side(&kept, &none), &side(&all, &none), Some((&all, &all)));
        assert_eq!(p.remove_from_b.len(), 20);
        assert!(p.hold_removals);
        // Held, so the next sync asks the same again.
        let (next_a, next_b) = after(&kept, &all, &p);
        assert_eq!(next_b.len(), 40);
        let again = plan(&side(&kept, &none), &side(&all, &none), Some((&next_a, &next_b)));
        assert_eq!(again.remove_from_b, p.remove_from_b);
        assert!(again.hold_removals);
    }

    #[test]
    fn held_removals_kept_are_not_asked_about_again() {
        let none = HashSet::new();
        let all: Vec<String> = (0..40).map(|n| n.to_string()).collect();
        let short: Vec<String> = all[..20].to_vec();
        let p = plan(&side(&short, &none), &side(&all, &none), Some((&all, &all)));
        let (next_a, next_b) = after(&short, &all, &p);
        let base = kept((&next_a, &next_b), &p.remove_from_a, &p.remove_from_b);
        let again = plan(&side(&short, &none), &side(&all, &none), Some((&base.0, &base.1)));
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn held_removals_agreed_to_are_settled() {
        let none = HashSet::new();
        let all: Vec<String> = (0..40).map(|n| n.to_string()).collect();
        let short: Vec<String> = all[..20].to_vec();
        let p = plan(&side(&short, &none), &side(&all, &none), Some((&all, &all)));
        let (next_a, next_b) = after(&short, &all, &p);
        let base = agreed((&next_a, &next_b), &p.remove_from_a, &p.remove_from_b);
        // B has had them taken out.
        let again = plan(&side(&short, &none), &side(&short, &none), Some((&base.0, &base.1)));
        assert!(again.is_empty(), "{again:?}");
    }

    #[test]
    fn a_large_share_of_a_short_playlist_is_held_too() {
        let none = HashSet::new();
        let all = keys(&["1", "2", "3", "4", "5", "6", "7", "8"]);
        let kept = keys(&["1", "2", "3", "4"]);
        let p = plan(&side(&kept, &none), &side(&all, &none), Some((&all, &all)));
        assert!(p.hold_removals);
    }

    #[test]
    fn one_song_out_of_a_short_playlist_needs_no_asking() {
        let none = HashSet::new();
        let all = keys(&["1", "2"]);
        let kept = keys(&["1"]);
        let p = plan(&side(&kept, &none), &side(&all, &none), Some((&all, &all)));
        assert_eq!(p.remove_from_b, keys(&["2"]));
        assert!(!p.hold_removals);
    }

    #[test]
    fn a_side_that_cannot_be_written_only_gives() {
        let none = HashSet::new();
        let base = (keys(&["1", "2"]), keys(&["1", "2"]));
        let (a, b) = (keys(&["1", "3"]), keys(&["1", "2", "4"]));
        let made_by_spotify = Side { songs: &a, writable: false, only_here: &none };
        let p = plan(&made_by_spotify, &side(&b, &none), Some((&base.0, &base.1)));
        assert_eq!(p.add_to_b, keys(&["3"]));
        assert_eq!(p.remove_from_b, keys(&["2"]));
        assert!(p.add_to_a.is_empty() && p.remove_from_a.is_empty());
    }

    #[test]
    fn removed_on_one_side_but_added_again_on_the_other_stays() {
        let none = HashSet::new();
        let base = (keys(&["1", "2"]), keys(&["1"]));
        // A dropped 2; B, which lacked it, added it since.
        let (a, b) = (keys(&["1"]), keys(&["1", "2"]));
        let p = plan(&side(&a, &none), &side(&b, &none), Some((&base.0, &base.1)));
        assert!(p.remove_from_b.is_empty(), "{p:?}");
        assert_eq!(p.add_to_a, keys(&["2"]));
    }

    #[test]
    fn a_song_in_twice_is_sent_once() {
        let none = HashSet::new();
        let (a, b) = (keys(&["1", "1", "2"]), keys(&[]));
        let p = plan(&side(&a, &none), &side(&b, &none), None);
        assert_eq!(p.add_to_b, keys(&["1", "2"]));
    }
}
