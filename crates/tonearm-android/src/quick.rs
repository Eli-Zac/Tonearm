//! Quick access on the phone: a section at the top of Home, laid out as
//! YouTube Music's app lays out its own. The pins are the desktop's, kept in the core's
//! `quick_access` file and synced through the Tonearm account; what is
//! left of the grid fills with what you opened and played lately.

use std::collections::HashSet;

use tonearm_core::history;
use tonearm_core::innertube::model;
use tonearm_core::quick_access::{self, QuickAccess};

use crate::pages::{self, Item};
use crate::Tonearm;

/// The grid is pages of nine, three by three, and holds this many.
const MOST: usize = 27;

/// One tile of Quick access.
#[derive(uniffi::Record)]
pub struct QuickTile {
    pub item: Item,
    /// Pinned to Quick access, so its tile shows the pin.
    pub pinned: bool,
}

#[uniffi::export]
impl Tonearm {
    /// Quick access on Home: the pins first, in the order they were made,
    /// then the albums, artists and playlists you opened lately, then the
    /// songs you played. Empty when there is nothing yet.
    pub fn quick_access_tiles(&self) -> Vec<QuickTile> {
        let pins = QuickAccess::load();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<QuickTile> = Vec::new();
        let mut add = |item: &model::Item, pinned: bool, out: &mut Vec<QuickTile>| {
            let Some(key) = history::key_of(item) else { return };
            if out.len() < MOST && seen.insert(key) {
                out.push(QuickTile { item: pages::item(item), pinned });
            }
        };
        for pin in pins.items() {
            add(pin, true, &mut out);
        }
        let taste = self.taste.lock().unwrap();
        for opened in taste.recent(false, MOST) {
            if quick_access::can_pin(&opened) {
                add(&opened, false, &mut out);
            }
        }
        for played in taste.recent(true, MOST) {
            add(&played, false, &mut out);
        }
        out
    }

    /// Opens each pinned page again for its cover and title as they are now,
    /// as a pin's cover link can expire. Blocks on the network; once a
    /// start, after the first sync. True when a pin changed, so Home reads
    /// Quick access again and the change is synced.
    pub fn refresh_quick_access(&self) -> bool {
        quick_access::refresh(&self.client)
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
