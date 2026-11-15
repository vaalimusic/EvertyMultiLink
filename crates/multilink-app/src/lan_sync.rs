use std::io::{Read, Write};
#[cfg(not(target_os = "macos"))]
use std::net::UdpSocket;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;

pub struct IncomingConfig {
    pub config: String,
    pub reply: Sender<Result<ImportReceipt, String>>,
    pub metadata: TransferMetadata,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct TransferMetadata {
    #[serde(rename = "subName")]
    pub name: Option<String>,
    #[serde(rename = "subExpire")]
    pub expires: u64,
    #[serde(rename = "disabledSignatures")]
    pub disabled_signatures: Vec<String>,
    #[serde(rename = "disabledNames")]
    pub disabled_names: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImportReceipt {
    pub count: usize,
    pub warnings: Vec<String>,
}

pub struct LanSyncSession {
    pub url: String,
    pub pin: String,
    running: Arc<AtomicBool>,
}

impl LanSyncSession {
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

pub fn detect_lan_ip() -> String {
    #[cfg(target_os = "macos")]
    return macos_lan_ip().unwrap_or_else(|| "127.0.0.1".into());
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip().to_string();
                    if ip != "0.0.0.0" {
                        return ip;
                    }
                }
            }
        }
        "127.0.0.1".to_string()
    }
}

pub fn start_lan_sync(tx: Sender<IncomingConfig>) -> Result<LanSyncSession, String> {
    let pin = format!(
        "{:04}",
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            % 9000
            + 1000)
    );

    let (listener, port) = match TcpListener::bind("0.0.0.0:8899") {
        Ok(l) => (l, 8899),
        Err(_) => match TcpListener::bind("0.0.0.0:0") {
            Ok(l) => {
                let p = l.local_addr().map(|a| a.port()).unwrap_or(8899);
                (l, p)
            }
            Err(e) => return Err(format!("Не удалось привязать сетевой порт: {e}")),
        },
    };

    let ip = detect_lan_ip();
    if ip == "127.0.0.1" {
        return Err("Не найден адрес локальной сети. Подключите Mac к Wi‑Fi или Ethernet".into());
    }
    let url = format!("http://{ip}:{port}?pin={pin}");
    let running = Arc::new(AtomicBool::new(true));
    let running_thread = running.clone();
    let pin_expected = pin.clone();

    thread::spawn(move || {
        let _ = listener.set_nonblocking(true);
        while running_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let pin_check = pin_expected.clone();
                    let tx_cloned = tx.clone();
                    thread::spawn(move || {
                        handle_client(&mut stream, &pin_check, &tx_cloned);
                    });
                }
                Err(_) => {
                    thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    });

    Ok(LanSyncSession { url, pin, running })
}

fn handle_client(stream: &mut TcpStream, pin: &str, tx: &Sender<IncomingConfig>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(10)));
    let (header, body) = match read_request(stream) {
        Ok(request) => request,
        Err(error) => {
            send_json(stream, serde_json::json!({"ok":false,"error":error}));
            return;
        }
    };
    let first_line = header.lines().next().unwrap_or("");

    if first_line.starts_with("GET ") {
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Everty MultiLink • Передача на ПК</title>
<style>
body {{ background: #0D1117; color: #E6EDF3; font-family: -apple-system, BlinkMacSystemFont, Segoe UI, Roboto, sans-serif; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; padding: 20px; }}
.card {{ background: #161C24; border: 1px solid #272F3B; border-radius: 16px; padding: 28px; width: 100%; max-width: 480px; box-shadow: 0 12px 36px rgba(0,0,0,0.6); }}
h2 {{ margin-top: 0; color: #6FE6B4; font-size: 22px; font-weight: 700; }}
p {{ color: #8B949E; font-size: 14px; line-height: 1.5; }}
label {{ display: block; font-size: 13px; font-weight: 600; color: #8B949E; margin-top: 16px; margin-bottom: 6px; }}
input, textarea {{ width: 100%; background: #0D1117; border: 1px solid #272F3B; border-radius: 8px; color: #FFF; padding: 12px; font-size: 14px; box-sizing: border-box; }}
input:focus, textarea:focus {{ outline: none; border-color: #6FE6B4; }}
button {{ width: 100%; margin-top: 20px; background: #6FE6B4; color: #0D1117; font-weight: 700; border: none; border-radius: 8px; padding: 14px; font-size: 16px; cursor: pointer; }}
button:active {{ opacity: 0.8; }}
</style>
</head>
<body>
<div class="card">
  <h2>Everty MultiLink • Синхронизация</h2>
  <p>Вставьте подписку или VLESS-ссылки для отправки на ваш компьютер:</p>
  <form id="transfer" method="POST" action="/api/import">
    <label>PIN-код подтверждения:</label>
    <input type="text" name="pin" value="{pin}" required>
    <label>Ссылка на подписку или VLESS:</label>
    <textarea name="config" rows="6" placeholder="https://... или vless://..." required></textarea>
    <button type="submit">Отправить на ПК</button>
  </form>
  <p id="result" role="status" aria-live="polite"></p>
</div>
<script>
document.getElementById('transfer').addEventListener('submit', async (event) => {{
  event.preventDefault();
  const form = event.target;
  const button = form.querySelector('button');
  const result = document.getElementById('result');
  button.disabled = true;
  result.textContent = 'Проверка и импорт на Mac…';
  try {{
    const response = await fetch('/api/import', {{method: 'POST', headers: {{'Content-Type': 'application/json'}},
      body: JSON.stringify({{pin: form.elements.pin.value, config: form.elements.config.value}})}});
    const data = await response.json();
    result.textContent = data.ok ? data.message : data.error;
    result.style.color = data.ok ? (data.warnings.length ? '#F1C25C' : '#6FE6B4') : '#FF9B9B';
    if (data.ok) form.elements.config.value = '';
  }} catch (error) {{
    result.textContent = 'Нет подтверждения от Mac. Проверьте результат в приложении и доступ к локальной сети.';
  }} finally {{ button.disabled = false; }}
}});
</script>
</body>
</html>"#
        );
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            html.len(),
            html
        );
        let _ = stream.write_all(resp.as_bytes());
    } else if first_line.starts_with("POST ") {
        let body_str = match std::str::from_utf8(&body) {
            Ok(body) => body,
            Err(_) => {
                send_json(
                    stream,
                    serde_json::json!({"ok":false,"error":"Конфигурация не в UTF-8"}),
                );
                return;
            }
        };

        let mut req_pin = String::new();
        let mut config = String::new();
        let mut metadata = TransferMetadata::default();

        if body_str.starts_with('{') {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body_str) {
                req_pin = val
                    .get("pin")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                config = val
                    .get("config")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match serde_json::from_value::<TransferMetadata>(val) {
                    Ok(value)
                        if value.disabled_names.len() <= 2000
                            && value.disabled_signatures.len() <= 2000 =>
                    {
                        metadata = value
                    }
                    _ => {
                        send_json(
                            stream,
                            serde_json::json!({"ok":false,"error":"Некорректная информация подписки"}),
                        );
                        return;
                    }
                }
            }
        } else {
            for pair in body_str.split('&') {
                let mut kv = pair.splitn(2, '=');
                let k = kv.next().unwrap_or("");
                let v = kv.next().unwrap_or("");
                let decoded = url_decode(v);
                if k == "pin" {
                    req_pin = decoded;
                } else if k == "config" {
                    config = decoded;
                }
            }
        }

        if req_pin.trim() != pin.trim() {
            send_json(
                stream,
                serde_json::json!({"ok":false,"error":"Неверный PIN-код"}),
            );
        } else if config.trim().is_empty() {
            send_json(
                stream,
                serde_json::json!({"ok":false,"error":"Конфигурация пуста"}),
            );
        } else {
            let (reply, result) = mpsc::channel();
            if tx
                .send(IncomingConfig {
                    config,
                    reply,
                    metadata,
                })
                .is_err()
            {
                send_json(
                    stream,
                    serde_json::json!({"ok":false,"error":"Окно приёма закрыто"}),
                );
                return;
            }
            if stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").is_err() { return; }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(55);
            let response = loop {
                match result.recv_timeout(std::time::Duration::from_secs(3)) {
                    Ok(Ok(receipt)) => {
                        let mut message = format!("Импортировано серверов: {}", receipt.count);
                        if !receipt.warnings.is_empty() {
                            message.push_str(&format!(
                                " · без фрагментации FM: {}",
                                receipt.warnings.len()
                            ));
                        }
                        break serde_json::json!({"ok":true,"count":receipt.count,"message":message,"warnings":receipt.warnings});
                    }
                    Ok(Err(error)) => break serde_json::json!({"ok":false,"error":error}),
                    Err(mpsc::RecvTimeoutError::Timeout)
                        if std::time::Instant::now() < deadline =>
                    {
                        // Legacy Android clients have an 8-second read timeout.
                        // JSON whitespace keeps the transfer alive without
                        // announcing success before the Mac commits the import.
                        if stream.write_all(b"1\r\n \r\n").is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        break serde_json::json!({"ok":false,"error":"Не получено подтверждение импорта. Проверьте результат на Mac"})
                    }
                }
            };
            let body = response.to_string();
            let _ = write!(stream, "{:x}\r\n{}\r\n0\r\n\r\n", body.len(), body);
        }
    }
}

fn url_decode(s: &str) -> String {
    let replaced = s.replace('+', " ");
    let mut result = Vec::new();
    let bytes = replaced.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(val) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                result.push(val);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

fn send_json(stream: &mut impl Write, value: serde_json::Value) {
    let body = value.to_string();
    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
}

fn read_request(stream: &mut impl Read) -> Result<(String, Vec<u8>), String> {
    const MAX_BODY: usize = 12 * 1024 * 1024;
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            if at > 16 * 1024 {
                return Err("Слишком большой HTTP-заголовок".into());
            }
            break at + 4;
        }
        if bytes.len() > 16 * 1024 {
            return Err("Слишком большой HTTP-заголовок".into());
        }
        let mut chunk = [0; 4096];
        let count = stream
            .read(&mut chunk)
            .map_err(|_| "Ошибка чтения запроса")?;
        if count == 0 {
            return Err("Запрос получен не полностью".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    };
    let header = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| "Некорректный HTTP-заголовок")?
        .to_owned();
    let mut length = None;
    for line in header.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err("Передача без Content-Length не поддерживается".into());
            }
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err("Повторный Content-Length".into());
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "Некорректная длина запроса")?,
                );
            }
        }
    }
    if header.starts_with("POST ") && length.is_none() {
        return Err("Нет длины конфигурации".into());
    }
    let length = length.unwrap_or(0);
    if length > MAX_BODY {
        return Err("Конфигурация слишком большая".into());
    }
    let mut body = bytes.split_off(header_end);
    body.truncate(length);
    if body.len() < length {
        let missing = length - body.len();
        stream
            .take(missing as u64)
            .read_to_end(&mut body)
            .map_err(|_| "Ошибка чтения конфигурации")?;
    }
    if body.len() != length {
        return Err("Конфигурация получена не полностью. Повторите отправку".into());
    }
    Ok((header, body))
}

#[cfg(target_os = "macos")]
fn macos_lan_ip() -> Option<String> {
    use std::{ffi::CStr, net::Ipv4Addr};
    struct Interfaces(*mut libc::ifaddrs);
    impl Drop for Interfaces {
        fn drop(&mut self) {
            unsafe { libc::freeifaddrs(self.0) }
        }
    }
    let mut head = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return None;
    }
    let interfaces = Interfaces(head);
    let mut candidates = Vec::new();
    let mut next = interfaces.0;
    while !next.is_null() {
        // getifaddrs owns every address and name until the list is freed above.
        let interface = unsafe { &*next };
        next = interface.ifa_next;
        if interface.ifa_addr.is_null()
            || interface.ifa_name.is_null()
            || interface.ifa_flags & libc::IFF_UP as u32 == 0
        {
            continue;
        }
        let name = unsafe { CStr::from_ptr(interface.ifa_name) }.to_string_lossy();
        if unsafe { (*interface.ifa_addr).sa_family as i32 } != libc::AF_INET {
            continue;
        }
        let address = unsafe { &*interface.ifa_addr.cast::<libc::sockaddr_in>() };
        let ip = Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes());
        if is_physical_lan_address(&name, ip) {
            candidates.push((!ip.is_private(), name != "en0", name.into_owned(), ip));
        }
    }
    candidates.sort();
    candidates.first().map(|(_, _, _, ip)| ip.to_string())
}

#[cfg(any(test, target_os = "macos"))]
fn is_physical_lan_address(name: &str, ip: std::net::Ipv4Addr) -> bool {
    name.strip_prefix("en")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit()))
        && !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && ip.octets()[0] < 224
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sender_waits_for_actual_import_result() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (tx, rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handle_client(&mut stream, "1234", &tx);
        });
        let body = serde_json::json!({"pin":"1234", "config":"unsupported config"}).to_string();
        write!(
            client,
            "POST /api/import HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let request = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(request.config, "unsupported config");
        use std::io::BufRead;
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut client = std::io::BufReader::new(client);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            client.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        assert!(headers.contains("Transfer-Encoding: chunked"));
        let mut heartbeat = [0; 6];
        client.read_exact(&mut heartbeat).unwrap();
        assert_eq!(
            &heartbeat, b"1\r\n \r\n",
            "only whitespace may precede actual import result"
        );
        request
            .reply
            .send(Err("Нет совместимых узлов".into()))
            .unwrap();
        let mut length = String::new();
        client.read_line(&mut length).unwrap();
        let length = usize::from_str_radix(length.trim(), 16).unwrap();
        let mut body = vec![0; length];
        client.read_exact(&mut body).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"], "Нет совместимых узлов");
        server.join().unwrap();
    }
    #[test]
    fn lan_address_excludes_vpn_and_fake_ip_interfaces() {
        for (name, ip) in [
            ("utun4", "10.0.0.1"),
            ("tun0", "192.168.1.2"),
            ("en0", "240.0.0.2"),
            ("lo0", "127.0.0.1"),
        ] {
            assert!(!is_physical_lan_address(name, ip.parse().unwrap()));
        }
        assert!(is_physical_lan_address(
            "en0",
            "192.168.1.25".parse().unwrap()
        ));
        assert!(is_physical_lan_address("en7", "10.10.1.2".parse().unwrap()));
    }
    #[test]
    fn reads_fragmented_headers_and_complete_unicode_payload() {
        struct Fragments(std::io::Cursor<Vec<u8>>);
        impl Read for Fragments {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let count = buffer.len().min(3);
                self.0.read(&mut buffer[..count])
            }
        }
        let body = serde_json::json!({"pin":"1234", "config":"vless://test#Москва"}).to_string();
        let request = format!(
            "POST /api/import HTTP/1.1\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let (_, actual) =
            read_request(&mut Fragments(std::io::Cursor::new(request.into_bytes()))).unwrap();
        assert_eq!(actual, body.as_bytes());
        assert!(
            read_request(&mut &b"POST / HTTP/1.1\r\nContent-Length: 20\r\n\r\nshort"[..]).is_err()
        );
        assert!(
            read_request(&mut &b"POST / HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n"[..])
                .is_err()
        );
    }
}
