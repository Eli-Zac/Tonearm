//! Quick access on the phone: Home's speed dial, as YouTube Music's app
//! has one. The pins are the desktop's, kept in the core's
//! `quick_access` file and synced through the Tonearm account; what is
//! left of the grid fills with what you opened and played lately.

use std::collections::HashSet;

use tonearm_core::history;
use tonearm_core::innertube::model;
use tonearm_core::quick_access::{self, QuickAccess};

use crate::pages::{self, Item};
use crate::Tonearm;

/// The grid is pages of nine, three by three, and holds this many.
const DIAL_SIZE: usize = 27;

/// One tile of the speed dial.
#[derive(uniffi::Record)]
pub struct DialItem {
    pub item: Item,
    /// Pinned to Quick access, so its tile shows the pin.
    pub pinned: bool,
}

#[uniffi::export]
impl Tonearm {
    /// Home's speed dial: the pins first, in the order they were made,
    /// then the albums, artists and playlists you opened lately, then the
    /// songs you played. Empty when there is nothing yet.
    pub fn speed_dial(&self) -> Vec<DialItem> {
        let pins = QuickAccess::load();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<DialItem> = Vec::new();
        let mut add = |item: &model::Item, pinned: bool, out: &mut Vec<DialItem>| {
            let Some(key) = history::key_of(item) else { return };
            if out.len() < DIAL_SIZE && seen.insert(key) {
                out.push(DialItem { item: pages::item(item), pinned });
            }
        };
        for pin in pins.items() {
            add(pin, true, &mut out);
        }
        let taste = self.taste.lock().unwrap();
        for opened in taste.recent(false, DIAL_SIZE) {
            if quick_access::can_pin(&opened) {
                add(&opened, false, &mut out);
            }
        }
        for played in taste.recent(true, DIAL_SIZE) {
            add(&played, false, &mut out);
        }
        out
    }

    /// Whether an album, artist or playlist can be pinned to Quick access.
    pub fn can_pin(&self, item: Item) -> bool {
        quick_access::can_pin(&pages::core_item(&item))
    }

    pub fn is_pinned(&self, item: Item) -> bool {
        QuickAccess::load().contains(&pages::core_item(&item))
    }

    /// Pins an album, artist or playlist to Quick access, or unpins it,
    /// as the desktop's menu does. Says which, for a toast. The app syncs
    /// the settings after, so the pin reaches the account's other devices.
    pub fn toggle_pin(&self, item: Item) -> String {
        let core = pages::core_item(&item);
        let mut pins = QuickAccess::load();
        let added = pins.toggle(&core);
        QuickAccess::save(pins.serialize());
        if added {
            format!("Pinned {} to Quick access", item.title)
        } else {
            format!("Unpinned {} from Quick access", item.title)
        }
    }
}
