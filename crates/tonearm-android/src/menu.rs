//! An item's long-press menu: the core's lines, which the desktop's right
//! click menu offers too, in what Kotlin can read. Downloads are left out
//! until the phone has a Downloads screen.

use crate::pages::{self, Item};
use tonearm_core::menu as core;

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Debug)]
pub enum MenuIcon {
    Play,
    Shuffle,
    PlayNext,
    AddQueue,
    Artist,
    Album,
    Radio,
    Download,
    Trash,
    Heart,
    Open,
    Library,
}

/// What a line does, to the item its menu is for unless it says.
#[derive(uniffi::Enum, Clone, PartialEq, Debug)]
pub enum MenuAction {
    PlayNext,
    AddToQueue,
    /// Opens a page.
    Browse { id: String },
    SongRadio,
    ArtistRadio { id: String },
    ArtistShuffle { id: String },
    ToggleDownload,
    Like,
    /// Likes or unlikes the song playing, as Now Playing's heart does.
    LikePlaying,
    AddToPlaylist,
    /// An album's or playlist's songs, in order or shuffled.
    Play,
    Shuffle,
    DownloadAll,
    Library { playlist: String, save: bool },
    Follow { channel: String, follow: bool },
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct MenuLine {
    pub label: String,
    pub icon: MenuIcon,
    pub action: MenuAction,
}

pub fn lines(item: &Item, context: &core::Context) -> Vec<MenuLine> {
    core::lines(&pages::core_item(item), context)
        .into_iter()
        .map(|line| MenuLine {
            label: line.label,
            icon: icon(line.icon),
            action: action(line.action),
        })
        .collect()
}

fn icon(icon: core::Icon) -> MenuIcon {
    match icon {
        core::Icon::Play => MenuIcon::Play,
        core::Icon::Shuffle => MenuIcon::Shuffle,
        core::Icon::PlayNext => MenuIcon::PlayNext,
        core::Icon::AddQueue => MenuIcon::AddQueue,
        core::Icon::Artist => MenuIcon::Artist,
        core::Icon::Album => MenuIcon::Album,
        core::Icon::Radio => MenuIcon::Radio,
        core::Icon::Download => MenuIcon::Download,
        core::Icon::Trash => MenuIcon::Trash,
        core::Icon::Heart => MenuIcon::Heart,
        core::Icon::Open => MenuIcon::Open,
        core::Icon::Library => MenuIcon::Library,
    }
}

fn action(action: core::Action) -> MenuAction {
    match action {
        core::Action::PlayNext => MenuAction::PlayNext,
        core::Action::AddToQueue => MenuAction::AddToQueue,
        core::Action::Browse(id) => MenuAction::Browse { id },
        core::Action::SongRadio => MenuAction::SongRadio,
        core::Action::ArtistRadio(id) => MenuAction::ArtistRadio { id },
        core::Action::ArtistShuffle(id) => MenuAction::ArtistShuffle { id },
        core::Action::ToggleDownload => MenuAction::ToggleDownload,
        core::Action::Like => MenuAction::Like,
        core::Action::LikePlaying => MenuAction::LikePlaying,
        core::Action::AddToPlaylist => MenuAction::AddToPlaylist,
        core::Action::Play => MenuAction::Play,
        core::Action::Shuffle => MenuAction::Shuffle,
        core::Action::DownloadAll => MenuAction::DownloadAll,
        core::Action::Library { playlist, save } => MenuAction::Library { playlist, save },
        core::Action::Follow { channel, follow } => MenuAction::Follow { channel, follow },
    }
}
