use std::{io, path::PathBuf};

#[cfg(windows)]
pub fn open_project_site(url: &str) -> io::Result<()> {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let url: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if (result as isize) <= 32 {
        Err(io::Error::other("Не удалось открыть браузер"))
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
pub fn open_project_site(url: &str) -> io::Result<()> {
    Err(io::Error::other(format!("Откройте {url}")))
}

#[cfg(windows)]
pub fn routing_file_dialog(save: bool) -> io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut file = vec![0u16; 32768];
    if save {
        for (slot, ch) in file.iter_mut().zip("multilink-routes.json".encode_utf16()) {
            *slot = ch;
        }
    }
    let filter: Vec<u16> = "MultiLink routes (*.json)\0*.json\0\0"
        .encode_utf16()
        .collect();
    let extension: Vec<u16> = "json\0".encode_utf16().collect();
    let title: Vec<u16> = if save {
        "Экспорт маршрутов"
    } else {
        "Импорт маршрутов"
    }
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let mut dialog = OPENFILENAMEW::default();
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if ok == 0 {
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(io::Error::other(format!("Диалог файла: {error}")))
        };
    }
    let end = file.iter().position(|ch| *ch == 0).unwrap_or(file.len());
    Ok(Some(std::ffi::OsString::from_wide(&file[..end]).into()))
}

#[cfg(windows)]
pub fn settings_file_dialog(save: bool) -> io::Result<Option<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut file = vec![0u16; 32768];
    if save {
        for (slot, ch) in file.iter_mut().zip("multilink-backup-settings.json".encode_utf16()) {
            *slot = ch;
        }
    }
    let filter: Vec<u16> = "Everty MultiLink Settings (*.json)\0*.json\0\0"
        .encode_utf16()
        .collect();
    let extension: Vec<u16> = "json\0".encode_utf16().collect();
    let title: Vec<u16> = if save {
        "Экспорт всех настроек"
    } else {
        "Импорт всех настроек"
    }
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let mut dialog = OPENFILENAMEW::default();
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | if save {
            OFN_OVERWRITEPROMPT
        } else {
            OFN_FILEMUSTEXIST
        };
    let ok = unsafe {
        if save {
            GetSaveFileNameW(&mut dialog)
        } else {
            GetOpenFileNameW(&mut dialog)
        }
    };
    if ok == 0 {
        let error = unsafe { CommDlgExtendedError() };
        return if error == 0 {
            Ok(None)
        } else {
            Err(io::Error::other(format!("Диалог файла: {error}")))
        };
    }
    let end = file.iter().position(|ch| *ch == 0).unwrap_or(file.len());
    Ok(Some(std::ffi::OsString::from_wide(&file[..end]).into()))
}

#[cfg(not(windows))]
pub fn routing_file_dialog(_: bool) -> io::Result<Option<PathBuf>> {
    Err(io::Error::other("Диалог файлов доступен в Windows"))
}

#[cfg(not(windows))]
pub fn settings_file_dialog(_: bool) -> io::Result<Option<PathBuf>> {
    Err(io::Error::other("Диалог файлов доступен в Windows"))
}

#[cfg(windows)]
pub fn restart_elevated(connect_after_restart: bool, compact: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let exe: Vec<u16> = std::env::current_exe()?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let verb: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
    let args: Vec<u16> = elevation_arguments(connect_after_restart, compact)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            exe.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if (result as isize) <= 32 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
pub fn restart_elevated(_: bool, _: bool) -> io::Result<()> {
    Err(io::Error::other("TUN доступен только в Windows"))
}
pub fn elevation_arguments(connect: bool, compact: bool) -> String {
    format!(
        "--elevated-restart{}{}",
        if connect {
            " --connect-after-elevation"
        } else {
            ""
        },
        if compact {
            " --compact-after-elevation"
        } else {
            ""
        }
    )
}
pub fn system_locale() -> String {
    #[cfg(windows)]
    {
        let mut buffer = [0u16; 85];
        let ui_language = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        let mut len = unsafe {
            windows_sys::Win32::Globalization::LCIDToLocaleName(
                ui_language as u32,
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                0,
            )
        };
        if len <= 1 {
            len = unsafe {
                windows_sys::Win32::Globalization::GetUserDefaultLocaleName(
                    buffer.as_mut_ptr(),
                    buffer.len() as i32,
                )
            };
        }
        if len > 1 {
            return String::from_utf16_lossy(&buffer[..len as usize - 1]);
        }
    }
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "en".into())
}

#[cfg(windows)]
pub fn is_elevated() -> io::Result<bool> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut length = 0;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    };
    let error = io::Error::last_os_error();
    unsafe {
        CloseHandle(token);
    }
    if ok == 0 {
        Err(error)
    } else {
        Ok(elevation.TokenIsElevated != 0)
    }
}

#[cfg(windows)]
pub fn tun_adapter_up(name: &str) -> bool {
    use windows_sys::Win32::NetworkManagement::{
        IpHelper::{ConvertInterfaceAliasToLuid, GetIfEntry2, MIB_IF_ROW2},
        Ndis::{IfOperStatusUp, NET_LUID_LH},
    };
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut luid = NET_LUID_LH::default();
    if unsafe { ConvertInterfaceAliasToLuid(name.as_ptr(), &mut luid) } != 0 {
        return false;
    }
    let mut row = MIB_IF_ROW2::default();
    row.InterfaceLuid = luid;
    unsafe { GetIfEntry2(&mut row) == 0 && row.OperStatus == IfOperStatusUp }
}
#[cfg(not(windows))]
pub fn tun_adapter_up(_: &str) -> bool {
    false
}
#[cfg(not(windows))]
pub fn is_elevated() -> io::Result<bool> {
    Ok(false)
}

#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
pub mod proxy;
#[cfg(windows)]
pub use proxy::{handle_watchdog_args, recover_system_proxy, ChildJob, SystemProxyGuard};

#[cfg(not(windows))]
pub struct SystemProxyGuard;
#[cfg(not(windows))]
impl SystemProxyGuard {
    pub fn enable(_: u16) -> io::Result<Self> {
        Err(io::Error::other(
            "System Proxy пока доступен только в Windows",
        ))
    }
    pub fn restore(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[cfg(not(windows))]
pub fn recover_system_proxy() -> io::Result<bool> {
    Ok(false)
}
#[cfg(not(windows))]
pub fn handle_watchdog_args() -> bool {
    false
}
#[cfg(not(windows))]
pub struct ChildJob;
#[cfg(not(windows))]
impl ChildJob {
    pub fn attach(_: &std::process::Child) -> io::Result<Self> {
        Ok(Self)
    }
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Everty")
        .join("MultiLink")
}

#[cfg(windows)]
fn crypt(bytes: &[u8], encrypt: bool) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes
            .len()
            .try_into()
            .map_err(|_| io::Error::other("Too much data"))?,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // DPAPI allocation is copied before LocalFree, and input stays alive for the call.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}
#[cfg(not(windows))]
fn crypt(_: &[u8], _: bool) -> io::Result<Vec<u8>> {
    Err(io::Error::other(
        "Защищённое хранилище пока реализовано только для Windows",
    ))
}

pub fn save(bytes: &[u8]) -> io::Result<()> {
    let data = crypt(bytes, true)?;
    std::fs::create_dir_all(data_dir())?;
    let path = data_dir().join("state.dpapi");
    let tmp = data_dir().join("state.next");
    std::fs::write(&tmp, data)?;
    // Windows rename cannot replace an existing file. Keep a recovery copy.
    let backup = data_dir().join("state.previous");
    if path.exists() {
        std::fs::copy(&path, &backup)?;
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(tmp, path)
}
pub fn load() -> io::Result<Option<Vec<u8>>> {
    let path = data_dir().join("state.dpapi");
    let backup = data_dir().join("state.previous");
    let path = if path.exists() {
        path
    } else if backup.exists() {
        backup
    } else {
        return Ok(None);
    };
    crypt(&std::fs::read(path)?, false).map(Some)
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn dpapi_roundtrip_and_tamper_detection() {
        let secret = b"vless-test-credentials";
        let mut encrypted = super::crypt(secret, true).unwrap();
        assert_ne!(encrypted, secret);
        assert_eq!(super::crypt(&encrypted, false).unwrap(), secret);
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xff;
        assert!(super::crypt(&encrypted, false).is_err());
    }
}
