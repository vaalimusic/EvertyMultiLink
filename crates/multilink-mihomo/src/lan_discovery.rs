use std::{
    io,
    net::{IpAddr, Ipv4Addr, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PORT: u16 = 18993;
const REQUEST: &[u8] = b"EVERTY-LAN-1?";
const RESPONSE: &[u8] = b"EVERTY-LAN-1!";

pub struct DiscoveryHost(Arc<AtomicBool>, Option<JoinHandle<()>>);

impl DiscoveryHost {
    pub fn start(ip: Ipv4Addr) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT))?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = shutdown.clone();
        let thread = thread::Builder::new()
            .name("everty-lan-discovery".into())
            .spawn(move || {
                let mut bytes = [0; 64];
                while !flag.load(Ordering::Relaxed) {
                    if let Ok((size, peer)) = socket.recv_from(&mut bytes) {
                        if matches!(peer.ip(), IpAddr::V4(peer) if peer.is_private() && peer != ip)
                            && size == REQUEST.len() + 32
                            && bytes[..size].starts_with(REQUEST)
                        {
                            bytes[..RESPONSE.len()].copy_from_slice(RESPONSE);
                            let _ = socket.send_to(&bytes[..size], peer);
                        }
                    }
                }
            })?;
        Ok(Self(shutdown, Some(thread)))
    }
}

impl Drop for DiscoveryHost {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        if let Some(thread) = self.1.take() {
            let _ = thread.join();
        }
    }
}

pub fn expand_lan_link(input: &str) -> Result<String, String> {
    let Some(key) = input
        .trim()
        .strip_prefix("everty-lan://")
        .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return Ok(input.into());
    };
    let ip = super::lan_gateway_ip().ok_or("Подключитесь к той же локальной сети")?;
    let socket = UdpSocket::bind((ip, 0)).map_err(|_| "Не удалось открыть поиск LAN-хоста")?;
    socket
        .set_broadcast(true)
        .map_err(|_| "Не удалось включить поиск LAN-хоста")?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|_| "Не удалось настроить поиск LAN-хоста")?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let request = [REQUEST, nonce.as_bytes()].concat();
    let response = [RESPONSE, nonce.as_bytes()].concat();
    let mut found = std::collections::HashSet::new();
    let mut bytes = [0; 64];
    for _ in 0..2 {
        socket
            .send_to(&request, (Ipv4Addr::BROADCAST, PORT))
            .map_err(|_| "Не удалось отправить поиск LAN-хоста")?;
        let deadline = Instant::now() + Duration::from_millis(650);
        while Instant::now() < deadline {
            if let Ok((count, peer)) = socket.recv_from(&mut bytes) {
                if bytes[..count] == response
                    && peer.port() == PORT
                    && matches!(peer.ip(), IpAddr::V4(peer) if peer.is_private() && peer != ip)
                {
                    found.insert(peer.ip());
                }
            }
        }
    }
    if found.len() != 1 {
        return Err(if found.is_empty() {
            "LAN-хост не найден. Используйте полную ссылку с IP"
        } else {
            "Найдено несколько LAN-хостов. Используйте полную ссылку с IP"
        }
        .into());
    }
    let host = found.into_iter().next().unwrap();
    Ok(format!(
        "everty-lan://{key}@{host}:{}",
        multilink_core::LAN_PORT
    ))
}
