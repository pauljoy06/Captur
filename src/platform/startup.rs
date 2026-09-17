use std::{mem::size_of, slice};

use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::{
            LibraryLoader::GetModuleFileNameW,
            Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey,
                RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
            },
        },
    },
    core::w,
};

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: windows::core::PCWSTR = w!("ProofSnip");

struct OwnedKey(HKEY);

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // Safety: this wrapper owns the key returned by RegOpenKeyExW.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

pub fn is_enabled() -> Result<bool, String> {
    let key = open_run_key(KEY_QUERY_VALUE)?;
    // Safety: only value metadata is queried, with no output data buffer.
    let status = unsafe { RegQueryValueExW(key.0, VALUE_NAME, None, None, None, None) };
    if status == ERROR_SUCCESS {
        Ok(true)
    } else if status == ERROR_FILE_NOT_FOUND {
        Ok(false)
    } else {
        Err(status_error(
            "could not query Windows startup setting",
            status.0,
        ))
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let key = open_run_key(KEY_SET_VALUE)?;
    if !enabled {
        // Safety: key is open for value writes and VALUE_NAME is a static UTF-16 string.
        let status = unsafe { RegDeleteValueW(key.0, VALUE_NAME) };
        return if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(status_error(
                "could not remove ProofSnip from Windows startup",
                status.0,
            ))
        };
    }

    let executable = executable_path()?;
    let command = format!("\"{executable}\" --background");
    let utf16: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    // Safety: u16 is plain data, and this byte view lives through RegSetValueExW.
    let bytes = unsafe {
        slice::from_raw_parts(utf16.as_ptr().cast::<u8>(), utf16.len() * size_of::<u16>())
    };
    // Safety: key is open for value writes and both name/data buffers remain valid for the call.
    let status = unsafe { RegSetValueExW(key.0, VALUE_NAME, None, REG_SZ, Some(bytes)) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(status_error(
            "could not add ProofSnip to Windows startup",
            status.0,
        ))
    }
}

fn open_run_key(
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<OwnedKey, String> {
    let mut key = HKEY::default();
    // Safety: key receives an owned handle on success.
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
    if status == ERROR_SUCCESS {
        Ok(OwnedKey(key))
    } else {
        Err(status_error(
            "could not open Windows startup settings",
            status.0,
        ))
    }
}

fn executable_path() -> Result<String, String> {
    let mut buffer = vec![0_u16; 32_768];
    // Safety: buffer is writable and large enough for the documented Windows maximum path.
    let length = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("could not determine the ProofSnip executable path".into());
    }
    Ok(String::from_utf16_lossy(&buffer[..length]))
}

fn status_error(context: &str, code: u32) -> String {
    format!(
        "{context}: {}",
        std::io::Error::from_raw_os_error(code as i32)
    )
}
