//! Windows media keys and the system media overlay.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
use std::time::Duration;

pub enum Key {
    Toggle,
    Next,
    Previous,
}

pub struct Keys(MediaControls);

impl Keys {
    pub fn new(window: &slint::Window, on_key: impl Fn(Key) + Send + 'static) -> Option<Self> {
        let handle = window.window_handle();
        let RawWindowHandle::Win32(win32) = handle.window_handle().ok()?.as_raw() else {
            return None;
        };
        let hwnd = win32.hwnd.get() as *mut std::ffi::c_void;
        let config = PlatformConfig {
            dbus_name: "tonearm",
            display_name: "Tonearm",
            hwnd: Some(hwnd),
        };
        let mut controls = MediaControls::new(config).ok()?;
        controls
            .attach(move |event| match event {
                MediaControlEvent::Play | MediaControlEvent::Pause | MediaControlEvent::Toggle => {
                    on_key(Key::Toggle)
                }
                MediaControlEvent::Next => on_key(Key::Next),
                MediaControlEvent::Previous => on_key(Key::Previous),
                _ => {}
            })
            .ok()?;
        Some(Keys(controls))
    }

    pub fn track(&mut self, title: &str, artist: &str, seconds: f32) {
        let duration = (seconds > 0.0).then(|| Duration::from_secs_f32(seconds));
        let _ = self.0.set_metadata(MediaMetadata {
            title: Some(title),
            artist: Some(artist),
            duration,
            ..Default::default()
        });
    }

    pub fn playing(&mut self, playing: bool) {
        let state = if playing {
            MediaPlayback::Playing { progress: None }
        } else {
            MediaPlayback::Paused { progress: None }
        };
        let _ = self.0.set_playback(state);
    }
}
