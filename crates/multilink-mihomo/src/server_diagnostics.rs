use super::binary;
use multilink_core::Node;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    io::Read,
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointPreset {
    General,
    GoogleApi,
    Antigravity,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointStatus {
    Ok,       // HTTP 200, or expected API responses (404 for root api, 400 for invalid token)
    Limited,  // HTTP 401/403 or geographical limitation detected
    Failed,   // TLS handshake failure, connection reset, timeout, 5xx
}

impl EndpointStatus {
    pub fn badge_text(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Limited => "Ограничен",
            Self::Failed => "Ошибка",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointCheckResult {
    pub name: String,
    pub url: String,
    pub method: String,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub status: EndpointStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerDiagnosticReport {
    pub node_name: String,
    pub exit_ip: Option<String>,
    pub exit_country: Option<String>,
    pub endpoints: Vec<EndpointCheckResult>,
    pub antigravity_supported: bool,
    pub general_internet_ok: bool,
    pub summary: String,
}

struct ProbeInstance {
    port: u16,
    id: String,
    child: Option<Child>,
    dir: PathBuf,
}

impl Drop for ProbeInstance {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl ProbeInstance {
    fn start(node: &Node) -> Result<Self, String> {
        let binary = binary();
        if !binary.is_file() {
            return Err("Ядро Mihomo не найдено для проверки".into());
        }
        let reserved = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| "Нет свободного порта для проверки")?;
        let port = reserved
            .local_addr()
            .map_err(|_| "Нет порта для проверки")?
            .port();
        let reserved_ctl = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| "Нет свободного порта для контроллера")?;
        let ctl_port = reserved_ctl
            .local_addr()
            .map_err(|_| "Нет порта для контроллера")?
            .port();
        drop(reserved_ctl);
        let runtime = multilink_platform::data_dir().join("runtime");
        std::fs::create_dir_all(&runtime).map_err(|_| "Нет доступа к папке ядра")?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let dir = runtime.join(format!("diag-{id}"));
        std::fs::create_dir(&dir).map_err(|_| "Нет доступа к папке проверки")?;

        let mut proxy = serde_json::to_value(&node.proxy).map_err(|_| "Ошибка конфигурации узла")?;
        proxy["name"] = json!("DIAGNOSTIC-EXIT");

        let config = json!({
            "mixed-port": port, "external-controller": format!("127.0.0.1:{ctl_port}"), "bind-address": "127.0.0.1", "allow-lan": false,
            "mode": "rule", "log-level": "silent", "ipv6": false,
            "authentication": [format!("probe:{id}")],
            "proxies": [proxy], "rules": ["MATCH,DIAGNOSTIC-EXIT"],
            "profile": {"store-selected": false, "store-fake-ip": false}
        });

        let config_path = dir.join("diag.yaml");
        std::fs::write(&config_path, config.to_string())
            .map_err(|_| "Не удалось подготовить конфигурацию проверки")?;

        let mut command = Command::new(binary);
        command
            .arg("-d")
            .arg(&dir)
            .arg("-f")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }

        drop(reserved);
        let mut child = command
            .spawn()
            .map_err(|_| "Не удалось запустить изолированную проверку")?;

        let _job = multilink_platform::ChildJob::attach(&mut child)
            .map_err(|_| "Не удалось запустить процесс проверки")?;

        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if child
                .try_wait()
                .map_err(|_| "Проверка завершилась преждевременно")?
                .is_some()
            {
                return Err("Mihomo не принял тестовую конфигурацию узла".into());
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
                let _ = child.kill();
                return Err("Таймаут ожидания запуска тестового ядра".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        Ok(Self {
            port,
            id,
            child: Some(child),
            dir,
        })
    }
}

struct EndpointSpec {
    name: &'static str,
    url: &'static str,
    method: &'static str,
    body: Option<&'static [u8]>,
    is_antigravity: bool,
    is_google: bool,
    is_general: bool,
}

const ALL_SPECS: &[EndpointSpec] = &[
    EndpointSpec {
        name: "Google Search",
        url: "https://www.google.com/",
        method: "GET",
        body: None,
        is_antigravity: false,
        is_google: true,
        is_general: true,
    },
    EndpointSpec {
        name: "Cloud Code API",
        url: "https://cloudcode-pa.googleapis.com/",
        method: "GET",
        body: None,
        is_antigravity: true,
        is_google: true,
        is_general: false,
    },
    EndpointSpec {
        name: "AI Code API",
        url: "https://aicode.googleapis.com/",
        method: "GET",
        body: None,
        is_antigravity: true,
        is_google: true,
        is_general: false,
    },
    EndpointSpec {
        name: "Antigravity Daily API",
        url: "https://daily-cloudcode-pa.googleapis.com/",
        method: "GET",
        body: None,
        is_antigravity: true,
        is_google: true,
        is_general: false,
    },
    EndpointSpec {
        name: "Antigravity CodeAssist",
        url: "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
        method: "POST",
        body: Some(b"{}"),
        is_antigravity: true,
        is_google: true,
        is_general: false,
    },
    EndpointSpec {
        name: "Google OAuth Token",
        url: "https://oauth2.googleapis.com/token",
        method: "POST",
        body: Some(b"grant_type=invalid"),
        is_antigravity: true,
        is_google: true,
        is_general: false,
    },
    EndpointSpec {
        name: "IP Info & Country",
        url: "https://ipinfo.io/json",
        method: "GET",
        body: None,
        is_antigravity: false,
        is_google: false,
        is_general: true,
    },
    EndpointSpec {
        name: "General Web (example.com)",
        url: "https://example.com/",
        method: "GET",
        body: None,
        is_antigravity: false,
        is_google: false,
        is_general: true,
    },
];

/// Execute comprehensive endpoint diagnostics through the selected node.
pub fn run_node_diagnostics(
    node: &Node,
    preset: EndpointPreset,
) -> Result<ServerDiagnosticReport, String> {
    let probe = ProbeInstance::start(node)?;

    let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{}", probe.port))
        .map_err(|_| "Ошибка настройки тестового прокси")?
        .basic_auth("probe", &probe.id);

    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .proxy(proxy)
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("Ошибка создания HTTP клиента: {e}"))?;

    let specs: Vec<&EndpointSpec> = ALL_SPECS
        .iter()
        .filter(|s| match preset {
            EndpointPreset::General => s.is_general,
            EndpointPreset::GoogleApi => s.is_google,
            EndpointPreset::Antigravity => s.is_antigravity,
            EndpointPreset::All => true,
        })
        .collect();

    let mut results = Vec::new();
    let mut exit_ip = None;
    let mut exit_country = None;
    let mut antigravity_ok = true;
    let mut general_ok = true;

    for spec in specs {
        let start = Instant::now();
        let request_builder = match spec.method {
            "POST" => {
                let mut req = client.post(spec.url);
                if let Some(body) = spec.body {
                    req = req.body(body.to_vec());
                    if spec.url.contains("oauth2") {
                        req = req.header("Content-Type", "application/x-www-form-urlencoded");
                    } else {
                        req = req.header("Content-Type", "application/json");
                    }
                }
                req
            }
            _ => client.get(spec.url),
        };

        match request_builder.send() {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                let status_code = response.status().as_u16();

                let mut body_bytes = Vec::new();
                let _ = response.take(128 * 1024).read_to_end(&mut body_bytes);
                let body_str = String::from_utf8_lossy(&body_bytes).to_string();

                let is_location_blocked = body_str
                    .to_ascii_lowercase()
                    .contains("user location is not supported");

                let (status, detail) = if is_location_blocked {
                    if spec.is_antigravity {
                        antigravity_ok = false;
                    }
                    (
                        EndpointStatus::Limited,
                        format!("{status_code} — Регион не поддерживается Google"),
                    )
                } else if spec.url.contains("ipinfo.io") {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body_str) {
                        if let Some(ip) = val["ip"].as_str() {
                            exit_ip = Some(ip.to_string());
                        }
                        if let Some(country) = val["country"].as_str() {
                            exit_country = Some(country.to_string());
                        }
                    }
                    (EndpointStatus::Ok, format!("200 OK (IP: {})", exit_ip.as_deref().unwrap_or("?")))
                } else if spec.url.contains("oauth2") {
                    // With invalid grant_type, HTTP 400 is the expected successful TLS & API response
                    if status_code == 400 {
                        (EndpointStatus::Ok, "400 OK (OAuth API доступен)".into())
                    } else if (200..300).contains(&status_code) {
                        (EndpointStatus::Ok, format!("{status_code} OK"))
                    } else {
                        if spec.is_antigravity {
                            antigravity_ok = false;
                        }
                        (EndpointStatus::Failed, format!("Код {status_code}"))
                    }
                } else if spec.url.contains("loadCodeAssist") {
                    // POST {} without auth returns 400 or 401 or 403 when reachable
                    if [400, 401, 403].contains(&status_code) {
                        (EndpointStatus::Ok, format!("{status_code} OK (Шлюз доступен)"))
                    } else if (200..300).contains(&status_code) {
                        (EndpointStatus::Ok, format!("{status_code} OK"))
                    } else {
                        if spec.is_antigravity {
                            antigravity_ok = false;
                        }
                        (EndpointStatus::Failed, format!("Код {status_code}"))
                    }
                } else if (200..300).contains(&status_code) {
                    (EndpointStatus::Ok, format!("{status_code} OK"))
                } else if status_code == 404 && spec.url.ends_with(".googleapis.com/") {
                    // Root of Google private APIs returns 404 when TLS and network route are fully functional
                    (EndpointStatus::Ok, "404 OK (Маршрут активен)".into())
                } else if [401, 403].contains(&status_code) {
                    (EndpointStatus::Limited, format!("{status_code} Ограничено"))
                } else {
                    if spec.is_general {
                        general_ok = false;
                    }
                    if spec.is_antigravity {
                        antigravity_ok = false;
                    }
                    (EndpointStatus::Failed, format!("Ошибка сервиса: {status_code}"))
                };

                results.push(EndpointCheckResult {
                    name: spec.name.to_string(),
                    url: spec.url.to_string(),
                    method: spec.method.to_string(),
                    status_code: Some(status_code),
                    latency_ms,
                    status,
                    detail,
                });
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                if spec.is_general {
                    general_ok = false;
                }
                if spec.is_antigravity {
                    antigravity_ok = false;
                }
                results.push(EndpointCheckResult {
                    name: spec.name.to_string(),
                    url: spec.url.to_string(),
                    method: spec.method.to_string(),
                    status_code: None,
                    latency_ms,
                    status: EndpointStatus::Failed,
                    detail: format!("TLS/Сеть: {e}"),
                });
            }
        }
    }

    let summary = if antigravity_ok && general_ok {
        "Все сервисы и Antigravity API полностью доступны".into()
    } else if general_ok && !antigravity_ok {
        "Интернет работает, но Antigravity API заблокирован или недоступен".into()
    } else {
        "Проблемы с подключением через данный сервер".into()
    };

    Ok(ServerDiagnosticReport {
        node_name: node.name.clone(),
        exit_ip,
        exit_country,
        endpoints: results,
        antigravity_supported: antigravity_ok,
        general_internet_ok: general_ok,
        summary,
    })
}
