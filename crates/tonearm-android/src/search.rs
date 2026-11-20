//! The search box's list, as the desktop's: suggested searches with the
//! typed part apart so it can be bold, the first results of the real
//! search, and recent searches while the box is empty. The rules and
//! timings are `tonearm_core::suggest`'s, shared with the desktop.

use crate::pages::{self, Item};
use crate::{Tonearm, TonearmError};
use tonearm_core::suggest;

/// What the search box lists for what was typed.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct Suggestions {
    pub searches: Vec<SuggestedSearch>,
    /// The search's first results, each with the short line the list shows
    /// ("Song • Artist") as its subtitle.
    pub items: Vec<Item>,
}

/// A suggested search: `head` is the part that was typed, shown bold.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct SuggestedSearch {
    pub query: String,
    pub head: String,
    pub tail: String,
}

fn suggestions(typed: &str, found: &suggest::Suggested) -> Suggestions {
    let typed = typed.trim();
    Suggestions {
        searches: suggest::queries(found)
            .into_iter()
            .map(|query| {
                let (head, tail) = suggest::split_typed(&query, typed);
                SuggestedSearch { query, head, tail }
            })
            .collect(),
        items: suggest::items(&found.page)
            .iter()
            .map(|item| {
                let mut row = pages::item(item);
                row.subtitle = suggest::subtitle(item);
                row
            })
            .collect(),
    }
}

#[uniffi::export]
impl Tonearm {
    /// How long typing pauses before `suggestions` is worth asking.
    pub fn suggest_delay_ms(&self) -> u64 {
        suggest::DELAY.as_millis() as u64
    }

    /// The list for `query` if it was fetched a moment ago, at once.
    pub fn cached_suggestions(&self, query: String) -> Option<Suggestions> {
        let found = self.suggest.lock().unwrap().get(&query)?;
        Some(suggestions(&query, &found))
    }

    /// The list for `query`: the suggested searches and the search itself,
    /// asked for together, and kept so the results page opens at once.
    pub fn suggestions(&self, query: String) -> Result<Suggestions, TonearmError> {
        if let Some(found) = self.cached_suggestions(query.clone()) {
            return Ok(found);
        }
        let found = suggest::fetch(&self.client, query.trim())?;
        self.suggest.lock().unwrap().put(&query, found.clone());
        Ok(suggestions(&query, &found))
    }

    /// Recent searches, newest first, for an empty search box.
    pub fn recent_searches(&self) -> Vec<String> {
        self.recent.lock().unwrap().get_or_insert_with(suggest::load_recent).clone()
    }

    /// Puts a search at the top of the recent ones.
    pub fn remember_search(&self, query: String) {
        let text = {
            let mut recent = self.recent.lock().unwrap();
            suggest::remember(recent.get_or_insert_with(suggest::load_recent), &query)
        };
        suggest::save_recent(&text);
    }

    /// Everything a search finds: the top result, songs, albums, artists,
    /// playlists, in YouTube Music's shelves. What the list fetched while
    /// typing is used when it is fresh.
    pub fn search_page(&self, query: String) -> Result<pages::Page, TonearmError> {
        let cached = self.suggest.lock().unwrap().get(&query);
        if let Some(found) = cached.filter(|f| !f.page.shelves.is_empty()) {
            return Ok(pages::page(&found.page));
        }
        Ok(pages::page(&self.client.search(&query)?))
    }

    /// One tab of the results, as the desktop's: 0 all, 1 songs, 2 albums,
    /// 3 artists, 4 playlists, 5 podcasts.
    pub fn search_tab(&self, query: String, tab: u32) -> Result<pages::Page, TonearmError> {
        if tab == 0 {
            return self.search_page(query);
        }
        Ok(pages::page(&self.client.search_in(&query, tab as usize)?))
    }

    /// More search results, as the list is scrolled. Search continues
    /// through the search endpoint, not browse.
    pub fn search_more(&self, continuation: String) -> Result<pages::Page, TonearmError> {
        Ok(pages::page(&self.client.search_more(&continuation)?))
    }
}
