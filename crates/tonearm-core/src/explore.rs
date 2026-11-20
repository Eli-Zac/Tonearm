//! The Explore page: YouTube Music's own, with the region's top songs as a
//! short numbered chart.

use crate::innertube::model::{Item, Page, Shelf};
use crate::innertube::Client;
use crate::store;
use std::sync::Mutex;

/// The Explore shelf YouTube Music draws as a ranked chart.
pub const CHART_TITLE: &str = "Top songs";
/// How many of the chart Explore lists. Its "more" opens the whole chart.
pub const CHART_ROWS: usize = 10;

/// Whether `shelf` is Explore's top songs chart, listed with rank numbers.
pub fn is_chart(shelf: &Shelf) -> bool {
    shelf.title == CHART_TITLE && !shelf.carousel
}

/// The Explore page, with the top songs chart as a numbered list. Blocks.
///
/// Signed in, Explore sends the chart as a carousel of cards; YouTube Music
/// shows it as a numbered chart, so it becomes a list here. Signed out,
/// Explore leaves it out, so we find the region's chart playlist and list
/// its top songs ourselves. Mood and genre buttons (items with a colour)
/// are left out: Search lists them.
pub fn fetch(client: &Client) -> Result<Page, String> {
    let (page, chart) = std::thread::scope(|scope| {
        let chart = scope.spawn(|| chart_shelf(client));
        (client.browse("FEmusic_explore", None), chart.join().ok().flatten())
    });
    let mut page = page?;
    for shelf in &mut page.shelves {
        shelf.items.retain(|i| i.color.is_none());
    }
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
fn chart_shelf(client: &Client) -> Option<Shelf> {
    static CHART: Mutex<Option<String>> = Mutex::new(None);
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
    (!items.is_empty()).then(|| Shelf {
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
