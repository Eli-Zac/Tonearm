//! Discord Rich Presence on Android, through Discord's Social SDK.
//!
//! There is no local pipe on a phone. The SDK's native library talks to the
//! Discord app (which has to be installed and signed in) over a bound
//! service, and needs no sign in of its own. The Kotlin side loads the
//! library and hands it the activity (`DiscordSocialSdkInit.setEngineActivity`);
//! here we find the library already loaded and call its C API.

use crate::services::Presence;
use std::ffi::c_void;

const RTLD_NOW: i32 = 2;
/// Succeed only when the library is already loaded. A fresh load here would
/// skip the JNI setup the Kotlin side does and crash on first use.
const RTLD_NOLOAD: i32 = 4;

extern "C" {
    fn dlopen(name: *const u8, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const u8) -> *mut c_void;
}

/// Every SDK object is one pointer wide and opaque to us.
#[repr(C)]
struct Object {
    opaque: *mut c_void,
}

impl Object {
    fn new() -> Self {
        Object { opaque: std::ptr::null_mut() }
    }
}

/// UTF-8 bytes and a length. The SDK copies what it is given.
#[repr(C)]
struct Text {
    ptr: *mut u8,
    size: usize,
}

impl Text {
    fn of(s: &str) -> Self {
        Text { ptr: s.as_ptr() as *mut u8, size: s.len() }
    }
}

type Obj = *mut Object;
type Str = *mut Text;

/// Looks up `Discord_<name>` for each entry and keeps the function pointers.
macro_rules! api {
    ($($name:ident: $ty:ty),* $(,)?) => {
        #[allow(non_snake_case)]
        #[derive(Clone, Copy)]
        struct Api { $($name: $ty),* }

        impl Api {
            fn load() -> Option<Api> {
                // SAFETY: the names are NUL terminated, and each symbol has
                // the signature written next to it, from Discord's cdiscord.h.
                unsafe {
                    let handle = dlopen(b"libdiscord_partner_sdk.so\0".as_ptr(), RTLD_NOW | RTLD_NOLOAD);
                    if handle.is_null() {
                        return None;
                    }
                    Some(Api { $($name: {
                        let symbol = dlsym(handle, concat!("Discord_", stringify!($name), "\0").as_ptr());
                        if symbol.is_null() {
                            return None;
                        }
                        std::mem::transmute::<*mut c_void, $ty>(symbol)
                    }),* })
                }
            }
        }
    };
}

api! {
    RunCallbacks: unsafe extern "C" fn(),
    Client_Init: unsafe extern "C" fn(Obj),
    Client_Drop: unsafe extern "C" fn(Obj),
    Client_SetApplicationId: unsafe extern "C" fn(Obj, u64),
    Client_ClearRichPresence: unsafe extern "C" fn(Obj),
    Client_UpdateRichPresence: unsafe extern "C" fn(Obj, Obj, *const c_void, *const c_void, *mut c_void),
    Activity_Init: unsafe extern "C" fn(Obj),
    Activity_Drop: unsafe extern "C" fn(Obj),
    // The one setter that takes its text by value, not by pointer.
    Activity_SetName: unsafe extern "C" fn(Obj, Text),
    Activity_SetType: unsafe extern "C" fn(Obj, i32),
    Activity_SetStatusDisplayType: unsafe extern "C" fn(Obj, *mut i32),
    Activity_SetState: unsafe extern "C" fn(Obj, Str),
    Activity_SetDetails: unsafe extern "C" fn(Obj, Str),
    Activity_SetAssets: unsafe extern "C" fn(Obj, Obj),
    Activity_SetTimestamps: unsafe extern "C" fn(Obj, Obj),
    ActivityAssets_Init: unsafe extern "C" fn(Obj),
    ActivityAssets_Drop: unsafe extern "C" fn(Obj),
    ActivityAssets_SetLargeImage: unsafe extern "C" fn(Obj, Str),
    ActivityAssets_SetLargeText: unsafe extern "C" fn(Obj, Str),
    ActivityTimestamps_Init: unsafe extern "C" fn(Obj),
    ActivityTimestamps_Drop: unsafe extern "C" fn(Obj),
    ActivityTimestamps_SetStart: unsafe extern "C" fn(Obj, u64),
    ActivityTimestamps_SetEnd: unsafe extern "C" fn(Obj, u64),
}

/// `Activity` type 2 is Listening.
const LISTENING: i32 = 2;

pub(crate) struct Discord {
    api: Api,
    /// Boxed so the address the SDK knows stays put.
    client: Box<Object>,
}

impl Discord {
    /// None when the SDK is not loaded yet or the application id is not a number.
    pub fn connect(app: &str) -> Option<Self> {
        let id: u64 = app.trim().parse().ok()?;
        let api = Api::load()?;
        let mut client = Box::new(Object::new());
        // SAFETY: `client` is a fresh object that Client_Init fills in.
        unsafe {
            (api.Client_Init)(&mut *client);
            (api.Client_SetApplicationId)(&mut *client, id);
        }
        Some(Discord { api, client })
    }

    /// Shows the track, or clears the status when `presence` is None.
    pub fn set(&mut self, presence: Option<&Presence>) -> Option<()> {
        let api = self.api;
        let client: Obj = &mut *self.client;
        let Some(p) = presence else {
            // SAFETY: the client was initialised in `connect`.
            unsafe { (api.Client_ClearRichPresence)(client) };
            return Some(());
        };
        // SAFETY: each object is initialised before use and dropped after.
        // UpdateRichPresence copies the activity, so we drop ours at the end.
        // The texts live until then. No callback or user data is passed.
        unsafe {
            let mut activity = Object::new();
            (api.Activity_Init)(&mut activity);
            // Without a name, Discord shows its generic "game".
            (api.Activity_SetName)(&mut activity, Text::of("Tonearm"));
            (api.Activity_SetType)(&mut activity, LISTENING);
            let mut display = i32::from(p.display);
            (api.Activity_SetStatusDisplayType)(&mut activity, &mut display);
            let mut details = Text::of(&p.title);
            (api.Activity_SetDetails)(&mut activity, &mut details);
            let mut state = Text::of(&p.artist);
            (api.Activity_SetState)(&mut activity, &mut state);

            let mut times = Object::new();
            (api.ActivityTimestamps_Init)(&mut times);
            (api.ActivityTimestamps_SetStart)(&mut times, p.start);
            if let Some(end) = p.end {
                (api.ActivityTimestamps_SetEnd)(&mut times, end);
            }
            (api.Activity_SetTimestamps)(&mut activity, &mut times);
            (api.ActivityTimestamps_Drop)(&mut times);

            if !p.art.is_empty() {
                let mut assets = Object::new();
                (api.ActivityAssets_Init)(&mut assets);
                let mut image = Text::of(&p.art);
                (api.ActivityAssets_SetLargeImage)(&mut assets, &mut image);
                if !p.album.is_empty() {
                    let mut album = Text::of(&p.album);
                    (api.ActivityAssets_SetLargeText)(&mut assets, &mut album);
                }
                (api.Activity_SetAssets)(&mut activity, &mut assets);
                (api.ActivityAssets_Drop)(&mut assets);
            }

            (api.Client_UpdateRichPresence)(
                client,
                &mut activity,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
            );
            (api.Activity_Drop)(&mut activity);
        }
        Some(())
    }

    pub fn run_callbacks(&self) {
        // SAFETY: takes no arguments.
        unsafe { (self.api.RunCallbacks)() };
    }
}

impl Drop for Discord {
    fn drop(&mut self) {
        // SAFETY: the client was initialised in `connect` and is dropped once.
        unsafe {
            (self.api.Client_ClearRichPresence)(&mut *self.client);
            (self.api.Client_Drop)(&mut *self.client);
        }
    }
}
