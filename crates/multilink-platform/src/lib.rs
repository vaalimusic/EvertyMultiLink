use std::{io, path::PathBuf};

pub fn ensure_loopback_port_free(port: u16) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        // Match Go's listener semantics: a recently closed controller can be in
        // TIME_WAIT without any process still listening on that port.
        socket.set_reuse_address(true)?;
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        if let Err(error) = socket.bind(&address.into()) {
            // The previous listener belonged to the root helper. Darwin can
            // deny cross-UID TIME_WAIT reuse here even though the next root
            // listener can reuse it. A refused connection proves no listener
            // is present; Mihomo still performs the authoritative bind.
            if error.kind() != io::ErrorKind::AddrInUse
                || !std::net::TcpStream::connect_timeout(
                    &address,
                    std::time::Duration::from_millis(150),
                )
                .is_err_and(|error| error.kind() == io::ErrorKind::ConnectionRefused)
            {
                return Err(error);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::net::TcpListener::bind(("127.0.0.1", port))?;
    }
    Ok(())
}

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
#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn open_project_site(url: &str) -> io::Result<()> {
    Err(io::Error::other(format!("Откройте {url}")))
}

#[cfg(target_os = "macos")]
pub fn open_project_site(url: &str) -> io::Result<()> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(url)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Не удалось открыть браузер"))
    }
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
        for (slot, ch) in file
            .iter_mut()
            .zip("multilink-backup-settings.json".encode_utf16())
        {
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

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn routing_file_dialog(_: bool) -> io::Result<Option<PathBuf>> {
    Err(io::Error::other("Диалог файлов доступен в Windows"))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn settings_file_dialog(_: bool) -> io::Result<Option<PathBuf>> {
    Err(io::Error::other("Диалог файлов доступен в Windows"))
}

#[cfg(target_os = "macos")]
fn macos_file_dialog(save: bool, name: &str) -> io::Result<Option<PathBuf>> {
    let script = if save {
        "on run argv\ntry\nreturn POSIX path of (choose file name default name (item 1 of argv))\non error number -128\nreturn \"\"\nend try\nend run"
    } else {
        "on run argv\ntry\nreturn POSIX path of (choose file)\non error number -128\nreturn \"\"\nend try\nend run"
    };
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", script, name])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let path = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let path = path.trim_end_matches(['\r', '\n']);
    Ok((!path.is_empty()).then(|| PathBuf::from(path)))
}

#[cfg(target_os = "macos")]
pub fn routing_file_dialog(save: bool) -> io::Result<Option<PathBuf>> {
    macos_file_dialog(save, "multilink-routes.json")
}

#[cfg(target_os = "macos")]
pub fn settings_file_dialog(save: bool) -> io::Result<Option<PathBuf>> {
    macos_file_dialog(save, "multilink-settings.json")
}

#[cfg(target_os = "macos")]
pub fn application_file_dialog() -> io::Result<Option<PathBuf>> {
    use core_foundation::{
        bundle::CFBundle,
        string::CFString,
        url::{kCFURLPOSIXPathStyle, CFURL},
    };
    let script = "try\nreturn POSIX path of (choose application as alias)\non error number -128\nreturn \"\"\nend try";
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("Не удалось выбрать приложение macOS"));
    }
    let path = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let path = path.trim_end_matches(['\r', '\n']);
    if path.is_empty() {
        return Ok(None);
    }
    let url = CFURL::from_file_system_path(CFString::new(path), kCFURLPOSIXPathStyle, true);
    let bundle = CFBundle::new(url)
        .ok_or_else(|| io::Error::other("Не удалось открыть пакет приложения"))?;
    bundle
        .executable_url()
        .ok_or_else(|| io::Error::other("У приложения нет исполняемого файла"))?;
    Ok(Some(PathBuf::from(path).canonicalize()?))
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
    #[cfg(target_os = "macos")]
    {
        use core_foundation::{array::CFArray, base::TCFType, string::CFString};
        use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
        // Finder-launched apps do not inherit the shell's LANG. Read the user's
        // ordered language preferences directly from macOS instead.
        let raw = unsafe { CFLocaleCopyPreferredLanguages() };
        if !raw.is_null() {
            let languages = unsafe { CFArray::<CFString>::wrap_under_create_rule(raw) };
            if let Some(language) = languages.get(0) {
                return language.to_string();
            }
        }
    }
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
#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn tun_adapter_up(_: &str) -> bool {
    false
}
#[cfg(target_os = "macos")]
pub fn tun_adapter_up(name: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(name) else {
        return false;
    };
    unsafe { libc::if_nametoindex(name.as_ptr()) != 0 }
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

#[cfg(target_os = "macos")]
pub mod macos_network;
#[cfg(target_os = "macos")]
mod macos_proxy;
#[cfg(target_os = "macos")]
pub use macos_network::SystemProxyGuard;

#[cfg(all(not(windows), not(target_os = "macos")))]
pub struct SystemProxyGuard;
#[cfg(all(not(windows), not(target_os = "macos")))]
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
#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn handle_watchdog_args() -> bool {
    false
}
#[cfg(target_os = "macos")]
pub fn handle_watchdog_args() -> bool {
    macos_network::handle_helper_args()
}
#[cfg(all(not(windows), not(target_os = "macos")))]
pub struct ChildJob;
#[cfg(all(not(windows), not(target_os = "macos")))]
impl ChildJob {
    pub fn attach(_: &std::process::Child) -> io::Result<Self> {
        Ok(Self)
    }
}
#[cfg(target_os = "macos")]
pub struct ChildJob {
    _watchdog: Option<std::process::Child>,
}
#[cfg(target_os = "macos")]
impl ChildJob {
    pub fn attach(child: &std::process::Child) -> io::Result<Self> {
        let executable = std::env::current_exe()?;
        if executable
            .file_name()
            .is_none_or(|name| name != "multilink-app")
        {
            return Ok(Self { _watchdog: None }); // Library test/example host.
        }
        let expected = macos_network::process_path(child.id() as i32)
            .ok_or_else(|| io::Error::other("Не удалось проверить процесс ядра"))?;
        let watchdog = std::process::Command::new(executable)
            .arg("--macos-child-watchdog")
            .arg(child.id().to_string())
            .arg(expected)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(Self {
            _watchdog: Some(watchdog),
        })
    }
}

pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Library/Application Support/Everty/MultiLink");
    #[cfg(not(target_os = "macos"))]
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
#[cfg(all(not(windows), not(target_os = "macos")))]
fn crypt(_: &[u8], _: bool) -> io::Result<Vec<u8>> {
    Err(io::Error::other(
        "Защищённое хранилище пока реализовано только для Windows",
    ))
}

#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
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

// Store credentials directly in the user's Keychain; never write plaintext state.
#[cfg(target_os = "macos")]
pub fn save(bytes: &[u8]) -> io::Result<()> {
    security_framework::passwords::set_generic_password("ru.everty.multilink", "settings", bytes)
        .map_err(io::Error::other)
}

#[cfg(target_os = "macos")]
pub fn load() -> io::Result<Option<Vec<u8>>> {
    match security_framework::passwords::get_generic_password("ru.everty.multilink", "settings") {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.code() == -25300 => Ok(None), // errSecItemNotFound
        Err(error) => Err(io::Error::other(error)),
    }
}

#[cfg(windows)]
pub fn prevent_sleep(prevent: bool) {
    use windows_sys::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };
    unsafe {
        if prevent {
            // Keep system and background network alive while VPN is active.
            // Display is still allowed to sleep/turn off to save power.
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        } else {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn prevent_sleep(_prevent: bool) {}

#[cfg(target_os = "macos")]
pub fn prevent_sleep(prevent: bool) {
    use core_foundation::{base::TCFType, string::CFString};
    use core_foundation_sys::string::CFStringRef;
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            assertion: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(assertion: u32) -> i32;
    }
    static ASSERTION: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);
    let Ok(mut current) = ASSERTION.lock() else {
        return;
    };
    if prevent && current.is_none() {
        let kind = CFString::new("PreventUserIdleSystemSleep");
        let name = CFString::new("Everty MultiLink VPN");
        let mut id = 0;
        if unsafe {
            IOPMAssertionCreateWithName(
                kind.as_concrete_TypeRef(),
                255,
                name.as_concrete_TypeRef(),
                &mut id,
            )
        } == 0
        {
            *current = Some(id);
        }
    } else if !prevent {
        if let Some(id) = current.take() {
            unsafe {
                IOPMAssertionRelease(id);
            }
        }
    }
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
