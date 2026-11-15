//! Keeps secrets on disk: the sign in cookie and the service keys. On
//! Windows, DPAPI encrypts them so only your Windows account can read them.

use std::path::PathBuf;

fn path(name: &str) -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("tonearm")
            .join(format!("{name}.bin")),
    )
}

pub fn load(name: &str) -> Option<String> {
    let bytes = std::fs::read(path(name)?).ok()?;
    String::from_utf8(unprotect(&bytes)?).ok()
}

pub fn save(name: &str, value: &str) -> Result<(), String> {
    let path = path(name).ok_or("No config folder")?;
    let folder = path.parent().ok_or("No config folder")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let bytes = protect(value.as_bytes()).ok_or("Could not encrypt the data")?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn clear(name: &str) {
    if let Some(p) = path(name) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(windows)]
fn crypt(input: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };
    let source = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: both blobs are valid for the call. Windows allocates `out`,
    // we copy it and free it with LocalFree as the API requires.
    unsafe {
        use std::ptr::{null, null_mut};
        let ok = if encrypt {
            CryptProtectData(&source, null(), null(), null(), null(), 0, &mut out)
        } else {
            CryptUnprotectData(&source, null_mut(), null(), null(), null(), 0, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Some(bytes)
    }
}

#[cfg(windows)]
fn protect(input: &[u8]) -> Option<Vec<u8>> {
    crypt(input, true)
}
#[cfg(windows)]
fn unprotect(input: &[u8]) -> Option<Vec<u8>> {
    crypt(input, false)
}

#[cfg(not(windows))]
fn protect(input: &[u8]) -> Option<Vec<u8>> {
    Some(input.to_vec())
}
#[cfg(not(windows))]
fn unprotect(input: &[u8]) -> Option<Vec<u8>> {
    Some(input.to_vec())
}
