//! Player buttons under the taskbar preview, and the tray icon Tonearm sits
//! in while its window is closed. Both talk to the main window by hooking
//! its window procedure, so they need no window of their own.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, SetWindowSubclass, Shell_NotifyIconW, TaskbarList,
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    THBF_ENABLED, THBN_CLICKED, THB_FLAGS, THB_ICON, THB_TOOLTIP, THUMBBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyMenu, GetCursorPos,
    GetSystemMetrics, IsIconic, LoadImageW, PostMessageW, RegisterWindowMessageW,
    SetForegroundWindow, ShowWindow, TrackPopupMenu, HICON, ICONINFO, IMAGE_ICON,
    LR_DEFAULTCOLOR, MF_SEPARATOR, MF_STRING, SM_CXSMICON, SM_CYSMICON, SW_RESTORE,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_LBUTTONUP,
    WM_NULL, WM_RBUTTONUP,
};

pub enum Action {
    Previous,
    Toggle,
    Next,
    /// Bring the window back from the tray.
    Show,
    Quit,
}

const PREVIOUS: u32 = 1;
const TOGGLE: u32 = 2;
const NEXT: u32 = 3;
const SHOW: u32 = 4;
const QUIT: u32 = 5;

/// What the tray icon sends the window when it is clicked.
const TRAY_MESSAGE: u32 = WM_APP + 7;
const SUBCLASS_ID: usize = 0x7a0e;

struct Icons {
    previous: HICON,
    play: HICON,
    pause: HICON,
    next: HICON,
    app: HICON,
}

struct State {
    hwnd: HWND,
    taskbar: Option<ITaskbarList3>,
    icons: Icons,
    playing: bool,
    /// The tray tooltip: the song, or just the app's name.
    tip: String,
    in_tray: bool,
    /// Sent when the window's taskbar button appears, again each time it
    /// is shown, and when Explorer restarts.
    button_created: u32,
    /// Sent when Explorer restarts and the tray is empty again.
    tray_created: u32,
    /// The installer broadcasts this to close a running copy, which would
    /// otherwise hold on to the exe it has to replace.
    installer_quit: u32,
    on_action: Box<dyn Fn(Action)>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn hwnd_of(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    let RawWindowHandle::Win32(win32) = handle.window_handle().ok()?.as_raw() else {
        return None;
    };
    Some(HWND(win32.hwnd.get() as *mut _))
}

/// Hooks the window. Call it once the window is shown. `on_action` runs on
/// the UI thread, from inside the window procedure, so it should only
/// queue work.
pub fn attach(window: &slint::Window, on_action: impl Fn(Action) + 'static) -> bool {
    let Some(hwnd) = hwnd_of(window) else { return false };
    unsafe {
        // Winit has normally set up COM on this thread already.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let state = State {
            hwnd,
            taskbar: None,
            icons: make_icons(),
            playing: false,
            tip: "Tonearm".into(),
            in_tray: false,
            button_created: RegisterWindowMessageW(w!("TaskbarButtonCreated")),
            tray_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            installer_quit: RegisterWindowMessageW(w!("Tonearm.InstallerQuit")),
            on_action: Box::new(on_action),
        };
        STATE.with(|slot| *slot.borrow_mut() = Some(state));
        if !SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, 0).as_bool() {
            STATE.with(|slot| slot.borrow_mut().take());
            return false;
        }
    }
    // The button may already be on the taskbar, its message gone by.
    with_state(add_buttons);
    true
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            if let Some(state) = slot.as_mut() {
                f(state);
            }
        }
    });
}

/// Shows play or pause on the middle button and in the tray menu.
pub fn set_playing(playing: bool) {
    with_state(|state| {
        if state.playing != playing {
            state.playing = playing;
            update_toggle(state);
        }
    });
}

/// Sets the tray tooltip to the playing song.
pub fn set_song(title: &str, artist: &str) {
    with_state(|state| {
        state.tip = match (title.is_empty(), artist.is_empty()) {
            (true, _) => "Tonearm".into(),
            (false, true) => title.into(),
            (false, false) => format!("{title} \u{2013} {artist}"),
        };
        if state.in_tray {
            tray(state, NIM_MODIFY);
        }
    });
}

/// Puts the icon in the tray, or takes it out.
pub fn set_in_tray(shown: bool) {
    with_state(|state| {
        if state.in_tray != shown {
            state.in_tray = shown;
            tray(state, if shown { NIM_ADD } else { NIM_DELETE });
        }
    });
}

/// Brings the window to the front after it is shown again.
pub fn raise(window: &slint::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Takes the tray icon out. Call it before the app exits, or the icon
/// lingers until the mouse passes over it.
pub fn detach() {
    set_in_tray(false);
}

fn button(id: u32, icon: HICON, tip: &str) -> THUMBBUTTON {
    let mut button = THUMBBUTTON {
        dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
        iId: id,
        hIcon: icon,
        dwFlags: THBF_ENABLED,
        ..Default::default()
    };
    copy_wide(&mut button.szTip, tip);
    button
}

fn toggle_button(state: &State) -> THUMBBUTTON {
    if state.playing {
        button(TOGGLE, state.icons.pause, "Pause")
    } else {
        button(TOGGLE, state.icons.play, "Play")
    }
}

fn add_buttons(state: &mut State) {
    unsafe {
        // A fresh list each time: after Explorer restarts the old one is dead.
        let Ok(taskbar) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
        else {
            return;
        };
        if taskbar.HrInit().is_err() {
            return;
        }
        let buttons = [
            button(PREVIOUS, state.icons.previous, "Previous"),
            toggle_button(state),
            button(NEXT, state.icons.next, "Next"),
        ];
        // This fails when the buttons are already there, and then the
        // update keeps them current.
        if taskbar.ThumbBarAddButtons(state.hwnd, &buttons).is_err() {
            let _ = taskbar.ThumbBarUpdateButtons(state.hwnd, &buttons);
        }
        state.taskbar = Some(taskbar);
    }
}

fn update_toggle(state: &State) {
    if let Some(taskbar) = &state.taskbar {
        unsafe {
            let _ = taskbar.ThumbBarUpdateButtons(state.hwnd, &[toggle_button(state)]);
        }
    }
}

fn tray(state: &State, message: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE) {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: state.hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: TRAY_MESSAGE,
        hIcon: state.icons.app,
        ..Default::default()
    };
    copy_wide(&mut data.szTip, &state.tip);
    unsafe {
        let _ = Shell_NotifyIconW(message, &data);
    }
}

/// The tray's right-click menu. Returns the chosen item, or zero.
fn tray_menu(state: &State) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let toggle = if state.playing { w!("Pause") } else { w!("Play") };
        let _ = AppendMenuW(menu, MF_STRING, SHOW as usize, w!("Show Tonearm"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, TOGGLE as usize, toggle);
        let _ = AppendMenuW(menu, MF_STRING, NEXT as usize, w!("Next"));
        let _ = AppendMenuW(menu, MF_STRING, PREVIOUS as usize, w!("Previous"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, QUIT as usize, w!("Quit"));
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // Without this the menu stays open when the user clicks elsewhere.
        let _ = SetForegroundWindow(state.hwnd);
        let flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
        let chosen = TrackPopupMenu(menu, flags, at.x, at.y, 0, state.hwnd, None).0 as u32;
        let _ = PostMessageW(state.hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        chosen
    }
}

fn action_for(id: u32) -> Option<Action> {
    Some(match id {
        PREVIOUS => Action::Previous,
        TOGGLE => Action::Toggle,
        NEXT => Action::Next,
        SHOW => Action::Show,
        QUIT => Action::Quit,
        _ => return None,
    })
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let mut action = None;
    let mut handled = false;
    with_state(|state| {
        if message == WM_COMMAND && (wparam.0 >> 16) as u32 == THBN_CLICKED {
            action = action_for((wparam.0 & 0xffff) as u32);
            handled = true;
        } else if message == TRAY_MESSAGE {
            match (lparam.0 & 0xffff) as u32 {
                WM_LBUTTONUP => action = Some(Action::Show),
                WM_RBUTTONUP => action = action_for(tray_menu(state)),
                _ => {}
            }
            handled = true;
        } else if message == state.button_created {
            add_buttons(state);
        } else if message == state.tray_created && state.in_tray {
            tray(state, NIM_ADD);
        } else if message == state.installer_quit && installed() {
            action = Some(Action::Quit);
            handled = true;
        }
    });
    if let Some(action) = action {
        // Out of the borrow, in case the callback calls back in.
        STATE.with(|slot| {
            if let Ok(slot) = slot.try_borrow() {
                if let Some(state) = slot.as_ref() {
                    (state.on_action)(action);
                }
            }
        });
    }
    if handled {
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

/// Whether this copy is the installed one, with the uninstaller beside it.
/// A copy run from a build folder ignores the installer.
fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").exists()))
        .unwrap_or(false)
}

fn copy_wide(into: &mut [u16], text: &str) {
    let mut n = 0;
    for unit in text.encode_utf16().take(into.len() - 1) {
        into[n] = unit;
        n += 1;
    }
    into[n] = 0;
}

// ---------------------------------------------------------------- icons

/// Whether the taskbar is light, so its buttons need dark glyphs.
fn light_taskbar() -> bool {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    result.is_ok() && value == 1
}

fn make_icons() -> Icons {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let shade = if light_taskbar() { 0x20 } else { 0xff };
    let triangle = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        move |x: f32, y: f32| {
            let side = |p: (f32, f32), q: (f32, f32)| (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
            let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
            (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
        }
    };
    let rect = |x0: f32, x1: f32| move |x: f32, y: f32| x >= x0 && x <= x1 && (0.2..=0.8).contains(&y);
    let play = triangle((0.28, 0.18), (0.28, 0.82), (0.82, 0.5));
    let pause_l = rect(0.24, 0.42);
    let pause_r = rect(0.58, 0.76);
    let next_t = triangle((0.2, 0.2), (0.2, 0.8), (0.64, 0.5));
    let next_b = rect(0.66, 0.78);
    let prev_t = triangle((0.8, 0.2), (0.8, 0.8), (0.36, 0.5));
    let prev_b = rect(0.22, 0.34);
    Icons {
        previous: draw_icon(size, shade, |x, y| prev_t(x, y) || prev_b(x, y)),
        play: draw_icon(size, shade, play),
        pause: draw_icon(size, shade, |x, y| pause_l(x, y) || pause_r(x, y)),
        next: draw_icon(size, shade, |x, y| next_t(x, y) || next_b(x, y)),
        app: app_icon(),
    }
}

/// Draws a glyph, given as a test of points in the unit square, with four
/// by four samples a pixel for smooth edges.
fn draw_icon(size: i32, shade: u8, inside: impl Fn(f32, f32) -> bool) -> HICON {
    const SAMPLES: i32 = 4;
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    hits += inside(x, y) as i32;
                }
            }
            let alpha = (hits * 255 / (SAMPLES * SAMPLES)) as u8;
            let i = ((py * size + px) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[shade, shade, shade, alpha]);
        }
    }
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                // Negative: rows run top down.
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HICON::default();
        };
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: BOOL(1),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        })
        .unwrap_or_default();
        let _ = DeleteObject(color);
        let _ = DeleteObject(mask);
        icon
    }
}

/// The logo built into the exe, at the tray's size.
fn app_icon() -> HICON {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        LoadImageW(
            module,
            PCWSTR(1 as *const u16),
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        )
        .map(|handle| HICON(handle.0))
        .unwrap_or_default()
    }
}
