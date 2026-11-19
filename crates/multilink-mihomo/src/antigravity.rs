use super::{binary, Client};
use multilink_core::Node;
use serde_json::json;
use std::{
    io::Read,
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravityCheck {
    Blocked(u16),
    Responded(u16),
    Inconclusive(u16),
}

fn classify(status: u16, body: &str) -> AntigravityCheck {
    if body
        .to_ascii_lowercase()
        .contains("user location is not supported")
    {
        AntigravityCheck::Blocked(status)
    } else if (200..300).contains(&status) || status == 404 {
        // A POST to the root can return 404 even when Google is reachable.
        // This probe has no account credentials and cannot confirm sign-in.
        AntigravityCheck::Responded(status)
    } else {
        AntigravityCheck::Inconclusive(status)
    }
}

struct Probe {
    path: PathBuf,
    child: Option<Child>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// The probe has exactly one exit. It never uses the active routing rules,
/// modifies Windows proxy settings, or falls back to a direct connection.
pub fn check_antigravity(node: &Node) -> Result<AntigravityCheck, String> {
    check(node, "https://daily-cloudcode-pa.googleapis.com")
}

fn check(node: &Node, endpoint: &str) -> Result<AntigravityCheck, String> {
    let binary = binary();
    if !binary.is_file() {
        return Err("Проверка недоступна: ядро Mihomo не найдено".into());
    }
    let reserved = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "Нет свободного порта для проверки")?;
    let port = reserved
        .local_addr()
        .map_err(|_| "Нет порта для проверки")?
        .port();
    let reserved_ctl = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "Нет свободного порта для проверки")?;
    let ctl_port = reserved_ctl
        .local_addr()
        .map_err(|_| "Нет порта для проверки")?
        .port();
    drop(reserved_ctl);
    let dir = multilink_platform::data_dir().join("runtime");
    std::fs::create_dir_all(&dir).map_err(|_| "Нет доступа к папке ядра")?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = dir.join(format!("antigravity-{id}"));
    std::fs::create_dir(&dir).map_err(|_| "Нет доступа к папке проверки")?;
    let mut probe = Probe {
        path: dir.join("probe.yaml"),
        child: None,
    };
    let mut proxy = serde_json::to_value(&node.proxy).map_err(|_| "Ошибка конфигурации узла")?;
    proxy["name"] = json!("ANTIGRAVITY-EXIT");
    let config = json!({
        "mixed-port": port, "external-controller": format!("127.0.0.1:{ctl_port}"), "bind-address": "127.0.0.1", "allow-lan": false,
        "mode": "rule", "log-level": "silent", "ipv6": false,
        "authentication": [format!("probe:{id}")],
        "proxies": [proxy], "rules": ["MATCH,ANTIGRAVITY-EXIT"],
        "profile": {"store-selected": false, "store-fake-ip": false}
    });
    std::fs::write(&probe.path, config.to_string())
        .map_err(|_| "Не удалось подготовить проверку")?;
    let mut command = Command::new(binary);
    command
        .arg("-d")
        .arg(&dir)
        .arg("-f")
        .arg(&probe.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    drop(reserved);
    let child = probe.child.insert(
        command
            .spawn()
            .map_err(|_| "Не удалось запустить проверку")?,
    );
    let _job = multilink_platform::ChildJob::attach(child)
        .map_err(|_| "Не удалось запустить изолированную проверку")?;
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if child
            .try_wait()
            .map_err(|_| "Проверка остановилась")?
            .is_some()
        {
            return Err("Mihomo не принял конфигурацию проверки".into());
        }
        if TcpStream::connect_timeout(
            &(Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_millis(100),
        )
        .is_ok()
        {
            break;
        }
        if Instant::now() >= until {
            return Err("Mihomo не запустил проверку вовремя".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))
        .map_err(|_| "Ошибка прокси проверки")?
        .basic_auth("probe", &id);
    let client = Client::builder()
        .no_proxy()
        .proxy(proxy)
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Ошибка HTTP-клиента")?;
    let response = client
        .post(endpoint)
        .json(&json!({}))
        .send()
        .map_err(|_| "Нет ответа Google через выбранный сервер")?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    const MAX_BODY: u64 = 256 * 1024;
    response
        .take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Ответ Google оборвался")?;
    let outcome = classify(status, &String::from_utf8_lossy(&bytes));
    if bytes.len() as u64 > MAX_BODY && !matches!(outcome, AntigravityCheck::Blocked(_)) {
        return Ok(AntigravityCheck::Inconclusive(status));
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_google_region_denial_from_other_errors() {
        assert_eq!(
            classify(400, "User location is not supported"),
            AntigravityCheck::Blocked(400)
        );
        assert_eq!(
            classify(400, "Invalid request"),
            AntigravityCheck::Inconclusive(400)
        );
        assert_eq!(classify(404, "Not Found"), AntigravityCheck::Responded(404));
        assert_eq!(
            classify(503, "Unavailable"),
            AntigravityCheck::Inconclusive(503)
        );
    }

    #[test]
    fn post_reaches_the_selected_vless_exit_through_real_mihomo() {
        use std::io::Write;
        if !binary().is_file() {
            return;
        }
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        server.set_nonblocking(true).unwrap();
        let port = server.local_addr().unwrap().port();
        let upstream = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(24);
            let mut stream = loop {
                match server.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        break stream;
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until =>
                    {
                        std::thread::sleep(Duration::from_millis(30))
                    }
                    Err(e) => panic!("no connection to the chosen exit: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut header = [0; 22];
            stream.read_exact(&mut header).unwrap();
            assert_eq!(header[17], 0, "no VLESS extensions");
            assert_eq!(header[18..22], [1, 0, 80, 2], "TCP to port 80, domain");
            let mut len = [0];
            stream.read_exact(&mut len).unwrap();
            let mut host = vec![0; len[0] as usize];
            stream.read_exact(&mut host).unwrap();
            assert_eq!(host, b"probe.example.invalid");
            stream.write_all(&[0, 0]).unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"POST / HTTP/1.1\r\n"));
            let mut body = [0; 2];
            stream.read_exact(&mut body).unwrap();
            assert_eq!(&body, b"{}");
            let message = "User location is not supported";
            write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}", message.len()).unwrap();
        });
        let node = multilink_core::parse_subscription(&format!(
            "vless://00000000-0000-4000-8000-000000000001@127.0.0.1:{port}#US-test"
        ))
        .unwrap()
        .remove(0);
        let result = check(&node, "http://probe.example.invalid/");
        assert_eq!(result, Ok(AntigravityCheck::Blocked(400)));
        upstream.join().unwrap();
    }
}
