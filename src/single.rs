//! One Tonearm at a time. A second start, as from the Start menu while the
//! first sits in the tray, brings the first back instead and quits.
//!
//! The first copy holds a named mutex and waits on a named event. A later
//! copy finds the mutex taken, sets the event, and exits. Both names carry
//! the data folder, so a test copy with a `TONEARM_PROFILE` of its own runs
//! beside the real one.

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
};

/// The running copy's claim. Dropping it lets another copy start.
pub struct Instance {
    mutex: *mut c_void,
    event: *mut c_void,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Names for this profile's mutex and event.
fn names() -> (Vec<u16>, Vec<u16>) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    crate::store::data_dir().hash(&mut hasher);
    let tag = format!("{:016x}", hasher.finish());
    (
        wide(&format!("Local\\Tonearm-running-{tag}")),
        wide(&format!("Local\\Tonearm-show-{tag}")),
    )
}

/// Claims this copy as the one that runs. None when another copy already
/// does: it has been asked to show itself, and this one should quit.
pub fn claim() -> Option<Instance> {
    let (mutex_name, event_name) = names();
    // SAFETY: plain Win32 calls with valid, nul terminated names. Handles
    // are closed in Drop, or here when this copy gives way.
    unsafe {
        let mutex = CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr());
        if mutex.is_null() {
            // No mutex at all: run rather than refuse to start.
            return Some(Instance { mutex, event: std::ptr::null_mut() });
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(mutex);
            let event = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
            if !event.is_null() {
                SetEvent(event);
                CloseHandle(event);
            }
            return None;
        }
        // Auto reset: each set wakes the listener once.
        let event = CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr());
        Some(Instance { mutex, event })
    }
}

impl Instance {
    /// Calls `show` on a thread of its own each time another copy starts.
    pub fn listen(&self, show: impl Fn() + Send + 'static) {
        if self.event.is_null() {
            return;
        }
        // The handle lives as long as the process: the claim is only
        // dropped as the app quits, and the thread goes with the process.
        let event = self.event as usize;
        let _ = std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || loop {
                // SAFETY: the event handle stays open, see above.
                let woke = unsafe { WaitForSingleObject(event as *mut c_void, INFINITE) };
                if woke != WAIT_OBJECT_0 {
                    return;
                }
                show();
            });
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: handles this claim opened, closed once. The mutex was
        // created owned, so closing it frees the name for the next copy.
        unsafe {
            if !self.mutex.is_null() {
                CloseHandle(self.mutex);
            }
        }
    }
}
