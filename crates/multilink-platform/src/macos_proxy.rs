//! Transactional per-service proxy settings, including active VPN services.
//! Runs only in the authorized helper.
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    data::CFData,
    dictionary::CFMutableDictionary,
    number::CFNumber,
    propertylist,
    string::CFString,
};
use core_foundation_sys::{
    base::{CFEqual, CFRelease},
    dictionary::CFDictionaryCreateMutableCopy,
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    ptr,
};
use system_configuration_sys::{network_configuration::*, preferences::*};

struct Preferences(SCPreferencesRef, bool);
impl Preferences {
    fn open() -> io::Result<Self> {
        let mut prefs = Self::read()?;
        if unsafe { SCPreferencesLock(prefs.0, 1) } == 0 {
            return Err(io::Error::other(
                "Не удалось заблокировать настройки сети macOS",
            ));
        }
        prefs.1 = true;
        Ok(prefs)
    }
    fn read() -> io::Result<Self> {
        let name = CFString::new("Everty MultiLink");
        let raw =
            unsafe { SCPreferencesCreate(ptr::null(), name.as_concrete_TypeRef(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::other("Не удалось открыть настройки сети macOS"));
        }
        Ok(Self(raw, false))
    }
    fn commit(&self) -> io::Result<()> {
        if unsafe { SCPreferencesCommitChanges(self.0) } == 0
            || unsafe { SCPreferencesApplyChanges(self.0) } == 0
        {
            return Err(io::Error::other("macOS не применила настройки сети"));
        }
        Ok(())
    }
}
impl Drop for Preferences {
    fn drop(&mut self) {
        unsafe {
            if self.1 {
                SCPreferencesUnlock(self.0);
            }
            CFRelease(self.0.cast());
        }
    }
}

/// Read-only baseline used by the real network integration check.
pub struct NetworkSnapshot {
    entries: Vec<Entry>,
}
impl NetworkSnapshot {
    pub fn capture() -> io::Result<Self> {
        let prefs = Preferences::read()?;
        let raw = unsafe { SCNetworkServiceCopyAll(prefs.0) };
        if raw.is_null() {
            return Err(io::Error::other("Не найдены службы сети"));
        }
        let services: CFArray = unsafe { CFArray::wrap_under_create_rule(raw) };
        let mut entries = Vec::new();
        for service in services.iter() {
            let service = (*service).cast();
            if unsafe { SCNetworkServiceGetEnabled(service) } == 0 {
                continue;
            }
            let interface = unsafe { SCNetworkServiceGetInterface(service) };
            if interface.is_null() {
                continue;
            }
            let name = unsafe { SCNetworkInterfaceGetBSDName(interface) };
            let physical = !name.is_null() && {
                let name = unsafe { CFString::wrap_under_get_rule(name) }.to_string();
                name.starts_with("en") || name.starts_with("bridge")
            };
            let id =
                unsafe { CFString::wrap_under_get_rule(SCNetworkServiceGetServiceID(service)) }
                    .to_string();
            for kind in ["Proxies", "DNS"] {
                if kind == "DNS" && !physical {
                    continue;
                }
                let protocol_type = CFString::new(kind);
                let protocol = unsafe {
                    SCNetworkServiceCopyProtocol(service, protocol_type.as_concrete_TypeRef())
                };
                if protocol.is_null() {
                    continue;
                }
                let _owned = unsafe { CFType::wrap_under_create_rule(protocol.cast()) };
                let config = unsafe { SCNetworkProtocolGetConfiguration(protocol) };
                entries.push(Entry {
                    service: id.clone(),
                    protocol: kind.into(),
                    enabled: unsafe { SCNetworkProtocolGetEnabled(protocol) } != 0,
                    original: if config.is_null() {
                        None
                    } else {
                        Some(serialize(config.cast())?)
                    },
                    applied: Vec::new(),
                });
            }
        }
        Ok(Self { entries })
    }
    pub fn is_unchanged(&self) -> io::Result<bool> {
        let current = Self::capture()?;
        if current.entries.len() != self.entries.len() {
            return Ok(false);
        }
        for entry in &self.entries {
            let Some(now) = current
                .entries
                .iter()
                .find(|now| now.service == entry.service && now.protocol == entry.protocol)
            else {
                return Ok(false);
            };
            if now.enabled != entry.enabled {
                eprintln!("network snapshot: {} enabled flag changed", entry.protocol);
                return Ok(false);
            }
            match (&entry.original, &now.original) {
                (Some(a), Some(b)) => {
                    if unsafe {
                        CFEqual(
                            deserialize(a)?.as_CFTypeRef(),
                            deserialize(b)?.as_CFTypeRef(),
                        )
                    } == 0
                    {
                        eprintln!(
                            "network snapshot: {} configuration changed ({} -> {} bytes)",
                            entry.protocol,
                            a.len(),
                            b.len()
                        );
                        return Ok(false);
                    }
                }
                (None, None) => {}
                _ => {
                    eprintln!(
                        "network snapshot: {} configuration presence changed ({} -> {})",
                        entry.protocol,
                        entry.original.is_some(),
                        now.original.is_some()
                    );
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

#[derive(Serialize, Deserialize)]
struct Entry {
    service: String,
    protocol: String,
    enabled: bool,
    original: Option<Vec<u8>>,
    applied: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    committed: bool,
    entries: Vec<Entry>,
}
pub struct ProxyTransaction {
    journal: Journal,
    path: PathBuf,
}

fn serialize(value: *const std::ffi::c_void) -> io::Result<Vec<u8>> {
    propertylist::create_data(value, propertylist::kCFPropertyListBinaryFormat_v1_0)
        .map(|data| data.bytes().to_vec())
        .map_err(|e| io::Error::other(e.to_string()))
}
fn deserialize(bytes: &[u8]) -> io::Result<CFType> {
    let (raw, _) = propertylist::create_with_data(
        CFData::from_buffer(bytes),
        propertylist::kCFPropertyListImmutable,
    )
    .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(unsafe { CFType::wrap_under_create_rule(raw) })
}
fn applied(original: *const std::ffi::c_void, port: u16) -> io::Result<Vec<u8>> {
    let mut dictionary: CFMutableDictionary<CFString, CFType> = if original.is_null() {
        CFMutableDictionary::new()
    } else {
        unsafe {
            CFMutableDictionary::wrap_under_create_rule(CFDictionaryCreateMutableCopy(
                ptr::null(),
                0,
                original.cast(),
            ))
        }
    };
    // Enabling an SCNetworkProtocol removes this marker from its dictionary.
    // Journal the effective enabled dictionary so conditional restore compares equal.
    dictionary.remove(CFString::new("__INACTIVE__"));
    for protocol in ["HTTP", "HTTPS", "SOCKS"] {
        dictionary.set(
            CFString::new(&format!("{protocol}Enable")),
            CFNumber::from(1i32).as_CFType(),
        );
        dictionary.set(
            CFString::new(&format!("{protocol}Proxy")),
            CFString::new("127.0.0.1").as_CFType(),
        );
        dictionary.set(
            CFString::new(&format!("{protocol}Port")),
            CFNumber::from(i32::from(port)).as_CFType(),
        );
    }
    for key in ["ProxyAutoConfigEnable", "ProxyAutoDiscoveryEnable"] {
        dictionary.set(CFString::new(key), CFNumber::from(0i32).as_CFType());
    }
    // Local controller and LAN sharing must never loop through the VPN proxy.
    let exceptions = CFArray::from_CFTypes(&[
        CFString::new("localhost"),
        CFString::new("127.0.0.1"),
        CFString::new("*.local"),
        CFString::new("10.0.0.0/8"),
        CFString::new("172.16.0.0/12"),
        CFString::new("192.168.0.0/16"),
    ]);
    dictionary.set(CFString::new("ExceptionsList"), exceptions.as_CFType());
    dictionary.set(
        CFString::new("ExcludeSimpleHostnames"),
        CFNumber::from(1i32).as_CFType(),
    );
    serialize(dictionary.as_CFTypeRef())
}

fn applied_dns(original: *const std::ffi::c_void) -> io::Result<Vec<u8>> {
    let mut dictionary: CFMutableDictionary<CFString, CFType> = if original.is_null() {
        CFMutableDictionary::new()
    } else {
        unsafe {
            CFMutableDictionary::wrap_under_create_rule(CFDictionaryCreateMutableCopy(
                ptr::null(),
                0,
                original.cast(),
            ))
        }
    };
    let servers = CFArray::from_CFTypes(&[
        CFString::new("198.18.0.2"),
        CFString::new("fdfe:dcba:9876::2"),
    ]);
    dictionary.set(CFString::new("ServerAddresses"), servers.as_CFType());
    serialize(dictionary.as_CFTypeRef())
}

impl ProxyTransaction {
    pub fn prepare(port: u16, path: &Path) -> io::Result<Self> {
        Self::prepare_for("Proxies", port, path)
    }
    pub fn prepare_dns(path: &Path) -> io::Result<Self> {
        Self::prepare_for("DNS", 1, path)
    }
    fn prepare_for(kind: &str, port: u16, path: &Path) -> io::Result<Self> {
        if port == 0 {
            return Err(io::Error::other("Некорректный порт прокси"));
        }
        let prefs = Preferences::open()?;
        let raw = unsafe { SCNetworkServiceCopyAll(prefs.0) };
        if raw.is_null() {
            return Err(io::Error::other("Не найдены сетевые службы macOS"));
        }
        let services: CFArray = unsafe { CFArray::wrap_under_create_rule(raw) };
        let mut entries = Vec::new();
        let protocol_type = CFString::new(kind);
        for service in services.iter() {
            let service = (*service).cast();
            if unsafe { SCNetworkServiceGetEnabled(service) } == 0 {
                continue;
            }
            let interface = unsafe { SCNetworkServiceGetInterface(service) };
            if interface.is_null() {
                continue;
            }
            let name = unsafe { SCNetworkInterfaceGetBSDName(interface) };
            let physical = !name.is_null() && {
                let name = unsafe { CFString::wrap_under_get_rule(name) }.to_string();
                name.starts_with("en") || name.starts_with("bridge")
            };
            if kind == "DNS" && !physical {
                continue;
            }
            let protocol = unsafe {
                SCNetworkServiceCopyProtocol(service, protocol_type.as_concrete_TypeRef())
            };
            if protocol.is_null() {
                continue;
            }
            let _owned = unsafe { CFType::wrap_under_create_rule(protocol.cast()) };
            let config = unsafe { SCNetworkProtocolGetConfiguration(protocol) };
            let id =
                unsafe { CFString::wrap_under_get_rule(SCNetworkServiceGetServiceID(service)) }
                    .to_string();
            entries.push(Entry {
                service: id,
                protocol: kind.into(),
                enabled: unsafe { SCNetworkProtocolGetEnabled(protocol) } != 0,
                original: if config.is_null() {
                    None
                } else {
                    Some(serialize(config.cast())?)
                },
                applied: if kind == "DNS" {
                    applied_dns(config.cast())?
                } else {
                    applied(config.cast(), port)?
                },
            });
        }
        if entries.is_empty() {
            return Err(io::Error::other(
                "Не найдены службы Wi‑Fi или Ethernet для системного прокси",
            ));
        }
        let transaction = Self {
            journal: Journal {
                committed: false,
                entries,
            },
            path: path.to_owned(),
        };
        transaction.persist()?;
        Ok(transaction)
    }
    fn persist(&self) -> io::Result<()> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let temporary = self.path.with_extension("next");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(&self.journal)?)?;
        file.sync_all()?;
        std::fs::rename(temporary, &self.path)
    }
    pub fn apply(&mut self) -> io::Result<()> {
        {
            let prefs = Preferences::open()?;
            for entry in &self.journal.entries {
                set_entry(&prefs, entry, false, false)?;
            }
            prefs.commit()?;
        }
        self.journal.committed = true;
        self.persist()
    }
    pub fn restore(&self) -> io::Result<()> {
        let prefs = Preferences::open()?;
        for entry in &self.journal.entries {
            set_entry(&prefs, entry, true, self.journal.committed)?;
        }
        prefs.commit()?;
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
    pub fn recover(path: &Path) -> io::Result<()> {
        match std::fs::read(path) {
            Ok(bytes) => Self {
                journal: serde_json::from_slice(&bytes)?,
                path: path.to_owned(),
            }
            .restore(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn set_entry(
    prefs: &Preferences,
    entry: &Entry,
    restore: bool,
    conditional: bool,
) -> io::Result<()> {
    let id = CFString::new(&entry.service);
    let service = unsafe { SCNetworkServiceCopy(prefs.0, id.as_concrete_TypeRef()) };
    if service.is_null() {
        return Ok(());
    } // Never resurrect a removed service.
    let _service = unsafe { CFType::wrap_under_create_rule(service.cast()) };
    let name = CFString::new(&entry.protocol);
    let protocol = unsafe { SCNetworkServiceCopyProtocol(service, name.as_concrete_TypeRef()) };
    if protocol.is_null() {
        return Ok(());
    }
    let _protocol = unsafe { CFType::wrap_under_create_rule(protocol.cast()) };
    let applied = deserialize(&entry.applied)?;
    if restore && conditional {
        let current = unsafe { SCNetworkProtocolGetConfiguration(protocol) };
        if current.is_null() || unsafe { CFEqual(current.cast(), applied.as_CFTypeRef()) } == 0 {
            return Ok(());
        }
    }
    let value = if restore {
        entry
            .original
            .as_ref()
            .map(|bytes| deserialize(bytes))
            .transpose()?
    } else {
        Some(applied)
    };
    let raw = value
        .as_ref()
        .map_or(ptr::null(), |value| value.as_CFTypeRef().cast());
    let enabled = if restore { entry.enabled } else { true };
    if unsafe { SCNetworkProtocolSetConfiguration(protocol, raw) } == 0
        || unsafe { SCNetworkProtocolSetEnabled(protocol, enabled as u8) } == 0
    {
        return Err(io::Error::other(
            "Не удалось изменить настройки системного прокси",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::dictionary::CFDictionary;
    #[test]
    fn proxy_conversion_keeps_snapshot_and_disables_pac_only_in_applied_copy() {
        let original = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("__INACTIVE__"),
                core_foundation::boolean::CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::new("ProxyAutoConfigEnable"),
                CFNumber::from(1i32).as_CFType(),
            ),
            (
                CFString::new("ProxyAutoConfigURLString"),
                CFString::new("https://pac.invalid/config").as_CFType(),
            ),
        ]);
        let bytes = serialize(original.as_CFTypeRef()).unwrap();
        let converted = applied(original.as_CFTypeRef(), 17890).unwrap();
        assert_eq!(
            serialize(original.as_CFTypeRef()).unwrap(),
            bytes,
            "original must remain immutable"
        );
        let value = deserialize(&converted).unwrap();
        let dictionary: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(value.as_CFTypeRef().cast()) };
        assert!(dictionary.find(&CFString::new("__INACTIVE__")).is_none());
        assert_eq!(
            dictionary
                .find(&CFString::new("HTTPProxy"))
                .unwrap()
                .downcast::<CFString>()
                .unwrap()
                .to_string(),
            "127.0.0.1"
        );
        assert_eq!(
            dictionary
                .find(&CFString::new("ProxyAutoConfigEnable"))
                .unwrap()
                .downcast::<CFNumber>()
                .unwrap()
                .to_i64(),
            Some(0)
        );
        assert_eq!(
            dictionary
                .find(&CFString::new("HTTPPort"))
                .unwrap()
                .downcast::<CFNumber>()
                .unwrap()
                .to_i64(),
            Some(17890)
        );
    }
    #[test]
    fn network_baseline_can_be_read_without_elevation() {
        let before = NetworkSnapshot::capture().unwrap();
        assert!(!before.entries.is_empty());
        assert!(before.is_unchanged().unwrap());
    }
    #[test]
    fn controller_port_check_rejects_live_listener_and_allows_immediate_reuse() {
        use std::{
            io::{Read, Write},
            net::{TcpListener, TcpStream},
        };
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        socket.set_reuse_address(true).unwrap();
        socket
            .bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        socket.listen(1).unwrap();
        let listener: TcpListener = socket.into();
        let address = listener.local_addr().unwrap();
        assert!(crate::ensure_loopback_port_free(address.port()).is_err());
        // The liveness check completes a TCP handshake without sending data.
        let (probe, _) = listener.accept().unwrap();
        drop(probe);
        let mut client = TcpStream::connect(address).unwrap();
        let (mut accepted, _) = listener.accept().unwrap();
        accepted.write_all(b"x").unwrap();
        client.read_exact(&mut [0; 1]).unwrap();
        accepted.shutdown(std::net::Shutdown::Both).unwrap();
        drop(accepted);
        let _ = client.read(&mut [0; 1]);
        drop(client);
        drop(listener);
        assert!(crate::ensure_loopback_port_free(address.port()).is_ok());
    }
}
