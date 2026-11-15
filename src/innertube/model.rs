//! Plain data types the rest of the app works with.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Song,
    Album,
    Artist,
    Playlist,
    #[default]
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub thumb: String,
    pub duration: String,
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
    pub browse_id: Option<String>,
    pub params: Option<String>,
}

impl Item {
    pub fn duration_secs(&self) -> f32 {
        let mut total = 0u32;
        for part in self.duration.split(':') {
            match part.trim().parse::<u32>() {
                Ok(n) => total = total * 60 + n,
                Err(_) => return 0.0,
            }
        }
        total as f32
    }
}

#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub title: String,
    /// Horizontal row of cards when true, vertical list when false.
    pub carousel: bool,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub title: String,
    pub subtitle: String,
    pub thumb: String,
    pub shelves: Vec<Shelf>,
    pub continuation: Option<String>,
}
