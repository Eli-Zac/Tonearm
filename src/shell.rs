//! Windows shell identity. The media flyout names an unpackaged app from its
//! AppUserModelID, and takes its icon from a shortcut with that ID. Tonearm
//! sets the ID on the process and keeps a Start menu shortcut that carries it.

use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    IShellLinkW, SetCurrentProcessExplicitAppUserModelID, ShellLink,
};

/// The ID shared by the process and the shortcut.
pub const APP_ID: &str = "Tonearm.Tonearm";

/// Gives this process the app ID. Call it before any window opens.
pub fn set_process_id() {
    let id = HSTRING::from(APP_ID);
    // Failure only means the flyout keeps its default name.
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(&id);
    }
}

/// Creates or fixes the Start menu shortcut on a background thread, so
/// startup does not wait on the shell. Does nothing when it already points
/// at this exe.
pub fn ensure_shortcut() {
    // A test copy with a profile of its own must not take the shortcut over.
    if std::env::var_os("TONEARM_PROFILE").is_some() {
        return;
    }
    std::thread::spawn(|| {
        let _ = ensure();
    });
}

fn ensure() -> Option<()> {
    let exe = std::env::current_exe().ok()?;
    let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
    let link = appdata.join(r"Microsoft\Windows\Start Menu\Programs\Tonearm.lnk");
    unsafe {
        // COM is per thread. A failed init (already set to another mode) is
        // still usable, so only undo a successful one.
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let done = if link.exists() && points_at(&link, &exe) {
            Some(())
        } else {
            std::fs::create_dir_all(link.parent()?).ok()?;
            write_link(&link, &exe).ok()
        };
        if init.is_ok() {
            CoUninitialize();
        }
        done
    }
}

/// Reads an existing shortcut and checks that it targets this exe and has
/// the app ID.
unsafe fn points_at(link: &Path, exe: &Path) -> bool {
    let check = || -> windows::core::Result<bool> {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = shell.cast()?;
        file.Load(&HSTRING::from(link.to_string_lossy().as_ref()), STGM_READ)?;
        let mut target = [0u16; 1024];
        shell.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        let target = String::from_utf16_lossy(&target);
        let target = target.trim_end_matches('\0');
        if !target.eq_ignore_ascii_case(&exe.to_string_lossy()) {
            return Ok(false);
        }
        let store: IPropertyStore = shell.cast()?;
        let id = store.GetValue(&PKEY_AppUserModel_ID)?;
        Ok(id.to_string() == APP_ID)
    };
    check().unwrap_or(false)
}

/// Writes the shortcut: target, description, and the app ID property.
unsafe fn write_link(link: &Path, exe: &Path) -> windows::core::Result<()> {
    let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    shell.SetPath(&HSTRING::from(exe.to_string_lossy().as_ref()))?;
    shell.SetDescription(&HSTRING::from("Tonearm"))?;
    let store: IPropertyStore = shell.cast()?;
    store.SetValue(&PKEY_AppUserModel_ID, &APP_ID.into())?;
    store.Commit()?;
    let file: IPersistFile = shell.cast()?;
    file.Save(PCWSTR(HSTRING::from(link.to_string_lossy().as_ref()).as_ptr()), true)?;
    Ok(())
}
