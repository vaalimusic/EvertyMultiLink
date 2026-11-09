use multilink_core::{Node, Policy, Settings};
mod antigravity;
pub use antigravity::*;
mod access;
pub use access::*;
pub mod zero_shadow;
pub use zero_shadow::*;
#[cfg(test)]
mod smart_tests;
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub const PORT: u16 = 17890;
pub const CONTROLLER: u16 = 19090;
const TUN_DEVICE: &str = "EvertyMultiLink";
pub fn binary() -> PathBuf {
    let name = if cfg!(windows) {
        "mihomo.exe"
    } else {
        "mihomo"
    };
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = std::env::current_exe().ok();
    let bundled = app.as_ref().map(|path| path.with_file_name(name));
    if let Some(path) = bundled.as_ref().filter(|path| path.is_file()) {
        return path.clone();
    }
    if let Some(path) = std::env::var_os("MULTILINK_MIHOMO").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if let (Some(app), Ok(workspace)) = (app, workspace.canonicalize()) {
        if app
            .canonicalize()
            .is_ok_and(|path| path.starts_with(&workspace))
        {
            return workspace.join("vendor/mihomo").join(name);
        }
    }
    bundled.unwrap_or_else(|| PathBuf::from(name))
}

fn runtime_proxy_names(
    nodes: &[&multilink_core::Node],
) -> std::collections::HashMap<String, String> {
    let mut names: std::collections::HashSet<_> = nodes.iter().map(|n| n.name.clone()).collect();
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let name = &node.name;
            let runtime = if multilink_core::server_route_target(name) != *name
                || matches!(
                    name.as_str(),
                    "GLOBAL" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE"
                ) {
                let mut alias = format!("EVERty-node-{index}");
                while !names.insert(alias.clone()) {
                    alias.push('-');
                }
                alias
            } else {
                name.clone()
            };
            (name.clone(), runtime)
        })
        .collect()
}

pub fn config(settings: &Settings, secret: &str) -> Result<String, String> {
    let mut effective = settings.clone();
    multilink_core::exclude_expired_access(&mut effective);
    let settings = &effective;
    let nodes: Vec<_> = settings.nodes.iter().filter(|n| n.enabled).collect();
    if nodes.is_empty() {
        return Err("Добавьте и включите хотя бы один сервер".into());
    }
    let proxy_names = runtime_proxy_names(&nodes);
    let mut proxies = Vec::new();
    for node in &nodes {
        let mut p = serde_json::to_value(&node.proxy).map_err(|_| "Ошибка конфигурации узла")?;
        p["name"] = json!(proxy_names[&node.name]);
        proxies.push(p);
    }
    // Keep Russian endpoints available for explicit COUNTRY-RU rules, but out
    // of the default pool so low-latency local nodes cannot capture VPN traffic.
    let mut default_nodes: Vec<&Node> = nodes
        .iter()
        .copied()
        .filter(|node| !multilink_core::is_russian_node(node))
        .collect();
    if default_nodes.is_empty() {
        default_nodes = nodes.clone();
    }
    if settings.policy == Policy::Smart {
        default_nodes.sort_by_key(|node| std::cmp::Reverse(node.quality.score().unwrap_or(50)));
    }
    let mut group = json!({"name":"MULTILINK", "type":"load-balance", "strategy":"round-robin", "proxies":default_nodes.iter().map(|n| &proxy_names[&n.name]).collect::<Vec<_>>(), "url":"https://www.gstatic.com/generate_204", "interval":60, "lazy":false});
    match settings.policy {
        Policy::Balance => {}
        Policy::Latency => {
            group["type"] = json!("url-test");
            group["tolerance"] = json!(50);
            group.as_object_mut().unwrap().remove("strategy");
        }
        Policy::Stable => {
            group["type"] = json!("fallback");
            group.as_object_mut().unwrap().remove("strategy");
        }
        Policy::Smart => {
            group["type"] = json!("url-test");
            group["tolerance"] = json!(settings.smart_tolerance.max(10));
            group["interval"] = json!(30);
            group["max-failed-times"] = json!(2);
            group["timeout"] = json!(4000);
            group.as_object_mut().unwrap().remove("strategy");
        }
    }
    let mut rules = multilink_core::normalize_direct_rules(&settings.rules)?;
    rules.push("MATCH,MULTILINK".into());
    let mut groups = vec![group];
    for (code, _) in multilink_core::available_countries(&settings.nodes)
        .into_iter()
        .filter(|(code, _)| {
            rules
                .iter()
                .any(|rule| rule.split(',').nth(2) == Some(format!("COUNTRY-{code}").as_str()))
        })
    {
        let name = format!("COUNTRY-{code}");
        let proxies: Vec<_> = nodes
            .iter()
            .filter(|n| multilink_core::node_country(&n.name).as_deref() == Some(code.as_str()))
            .map(|n| &proxy_names[&n.name])
            .collect();
        groups.push(json!({"name":name,"type":"load-balance","strategy":"round-robin","proxies":proxies,"url":"https://www.gstatic.com/generate_204","interval":60,"lazy":false}));
    }
    for rule in &mut rules {
        if let Some(target) = rule.split(',').nth(2) {
            let resolved = if matches!(target, "DIRECT" | "MULTILINK")
                || groups.iter().any(|g| g["name"] == target)
            {
                target
            } else {
                let node_name = multilink_core::server_route_name(target);
                proxy_names
                    .get(node_name.as_deref().unwrap_or(target))
                    .map(String::as_str)
                    .unwrap_or("MULTILINK")
            };
            *rule = format!("{},{resolved}", multilink_core::rule_key(rule));
        }
    }
    if settings.policy == Policy::Smart {
        let mut count = 0;
        for rule in &mut rules {
            let parts: Vec<_> = rule.split(',').collect();
            if parts.len() == 3
                && parts[0] == "DOMAIN-SUFFIX"
                && parts[2] == "MULTILINK"
                && parts[1].contains('.')
                && count < 32
            {
                let domain = parts[1].to_owned();
                let mut name = format!("MULTILINK-SITE-{count}");
                while proxy_names.values().any(|n| *n == name) {
                    name.push('-');
                }
                groups.push(json!({"name":name,"type":"url-test","proxies":default_nodes.iter().map(|n| &proxy_names[&n.name]).collect::<Vec<_>>(),"url":format!("https://{domain}/"),"interval":60,"timeout":4000,"tolerance":settings.smart_tolerance.max(10),"max-failed-times":2,"expected-status":"200-399/401/404","lazy":true}));
                *rule = format!("DOMAIN-SUFFIX,{domain},{name}");
                count += 1;
            }
        }
    }
    let mut config = json!({
        "mixed-port":PORT, "bind-address":"127.0.0.1", "allow-lan":false,
        "mode":"rule", "log-level":"silent", "ipv6":false,
        "external-controller":format!("127.0.0.1:{CONTROLLER}"), "secret":secret,
        "proxies":proxies, "proxy-groups":groups, "rules":rules,
        "profile":{"store-selected":false, "store-fake-ip":false},
    });
    if settings.tun || rules.iter().any(|rule| rule.starts_with("PROCESS-NAME,")) {
        config["find-process-mode"] = json!("strict");
    }
    if settings.tun {
        config["tun"] = json!({
            "enable": true,
            "device": TUN_DEVICE,
            "stack": "system",
            "auto-route": true,
            "auto-detect-interface": true,
            "strict-route": false,
            "dns-mode": "disabled",
            "dns-hijack": ["any:53", "tcp://any:53"],
            "route-exclude-address": ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]
        });
        config["dns"] = json!({
            "enable": true,
            "ipv6": false,
            "enhanced-mode": "fake-ip",
            "fake-ip-filter": ["*.lan", "*.local", "localhost"],
            "default-nameserver": ["1.1.1.1", "8.8.8.8"],
            "nameserver": ["https://1.1.1.1/dns-query", "https://8.8.8.8/dns-query"]
        });
        config["sniffer"] = json!({"enable": true, "sniff": {"HTTP": {"ports": [80, "8080-8880"], "override-destination": true}, "TLS": {"ports": [443, 8443]}, "QUIC": {"ports": [443, 8443]}}});
    }
    if rules
        .iter()
        .any(|rule| rule.starts_with("RULE-SET,RU-SITES,"))
    {
        config["rule-providers"] = json!({"RU-SITES":{"type":"file","behavior":"domain","format":"mrs","path":multilink_platform::data_dir().join("runtime/category-ru.mrs")}});
    }
    serde_yaml::to_string(&config).map_err(|_| "Не удалось создать конфигурацию".into())
}

#[derive(Clone, Default)]
pub struct Metrics {
    pub download: u64,
    pub upload: u64,
    pub connections: usize,
    pub down_rate: f64,
    pub up_rate: f64,
}

pub struct Engine {
    child: Child,
    client: Client,
    secret: String,
    path: PathBuf,
    last: Option<(Instant, u64, u64)>,
    _job: multilink_platform::ChildJob,
    settings: Settings,
}
#[derive(Default)]
pub struct RoutingChange {
    pub closed_connections: usize,
    pub retry_tabs: bool,
}
impl Engine {
    fn ensure_rule_data(settings: &Settings) -> Result<(), String> {
        if settings
            .rules
            .iter()
            .any(|rule| rule.starts_with("RULE-SET,RU-SITES,"))
        {
            let path = multilink_platform::data_dir().join("runtime/category-ru.mrs");
            let bytes = include_bytes!("../../../assets/category-ru.mrs");
            if std::fs::read(&path).ok().as_deref() != Some(bytes) {
                std::fs::create_dir_all(path.parent().ok_or("Нет папки набора правил")?)
                    .map_err(|_| "Нет доступа к папке набора правил")?;
                std::fs::write(&path, bytes)
                    .map_err(|_| "Не удалось сохранить набор российских доменов")?;
            }
        }
        Ok(())
    }
    pub fn start(settings: &Settings) -> Result<Self, String> {
        if settings.tun
            && !multilink_platform::is_elevated()
                .map_err(|e| format!("Не удалось проверить права Windows: {e}"))?
        {
            return Err(
                "Для TUN закройте приложение через трей и запустите его от имени администратора"
                    .into(),
            );
        }
        let binary = binary();
        if !binary.is_file() {
            return Err("Ядро Mihomo не найдено. Переустановите приложение или положите mihomo.exe рядом с ним".into());
        }
        if settings.tun && multilink_platform::tun_adapter_up(TUN_DEVICE) {
            return Err(
                "Адаптер EvertyMultiLink уже активен. Отключите другой экземпляр TUN".into(),
            );
        }
        // Fail if another application owns either port, before writing credentials.
        for port in [PORT, CONTROLLER] {
            std::net::TcpListener::bind(("127.0.0.1", port))
                .map_err(|_| format!("Локальный порт {port} уже занят"))?;
        }
        let dir = multilink_platform::data_dir().join("runtime");
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| "Ошибка HTTP-клиента")?;
        std::fs::create_dir_all(&dir).map_err(|_| "Нет доступа к папке ядра")?;
        let path = dir.join(format!("session-{}.yaml", uuid::Uuid::new_v4()));
        Self::ensure_rule_data(settings)?;
        let secret = uuid::Uuid::new_v4().to_string();
        std::fs::write(&path, config(settings, &secret)?)
            .map_err(|_| "Не удалось записать конфигурацию")?;
        let mut command = Command::new(binary);
        command
            .arg("-d")
            .arg(&dir)
            .arg("-f")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                return Err("Не удалось запустить Mihomo".into());
            }
        };
        let job = match multilink_platform::ChildJob::attach(&child) {
            Ok(job) => job,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&path);
                return Err("Не удалось привязать жизненный цикл Mihomo к приложению".into());
            }
        };
        let mut engine = Self {
            child,
            client,
            secret,
            path,
            last: None,
            _job: job,
            settings: settings.clone(),
        };
        let until = Instant::now() + Duration::from_secs(12);
        while Instant::now() < until {
            if engine.child.try_wait().ok().flatten().is_some() {
                return Err("Mihomo отклонил конфигурацию или завершился".into());
            }
            if engine.get("version").is_ok()
                && (!settings.tun || multilink_platform::tun_adapter_up(TUN_DEVICE))
            {
                return Ok(engine);
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Err(if settings.tun { "Не удалось запустить TUN-адаптер за 12 секунд. Проверьте права администратора и другие VPN" } else { "Ядро не ответило за 12 секунд" }.into())
    }
    fn get(&self, path: &str) -> Result<Value, String> {
        self.client
            .get(format!("http://127.0.0.1:{CONTROLLER}/{path}"))
            .bearer_auth(&self.secret)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|_| "Нет связи с локальным API Mihomo".to_owned())
    }
    fn apply_yaml(&mut self, contents: &str) -> Result<(), String> {
        let next = self
            .path
            .with_file_name(format!("session-{}.yaml", uuid::Uuid::new_v4()));
        std::fs::write(&next, contents).map_err(|_| "Не удалось подготовить правила для ядра")?;
        let result = self
            .client
            .put(format!("http://127.0.0.1:{CONTROLLER}/configs?force=false"))
            .bearer_auth(&self.secret)
            .json(&json!({"path":next}))
            .send();
        match result {
            Ok(response) if response.status().is_success() => {
                let old = std::mem::replace(&mut self.path, next);
                let _ = std::fs::remove_file(old);
                Ok(())
            }
            other => {
                // An HTTP validation error leaves the previous config active.
                // A transport error has an unknown outcome: explicitly reload
                // the previous known-good file before reporting failure.
                let rollback_ok = if other.is_err() {
                    self.client
                        .put(format!("http://127.0.0.1:{CONTROLLER}/configs?force=false"))
                        .bearer_auth(&self.secret)
                        .json(&json!({"path":self.path}))
                        .send()
                        .is_ok_and(|r| r.status().is_success())
                } else {
                    true
                };
                let _ = std::fs::remove_file(next);
                Err(if rollback_ok {
                    "Ядро не применило новые правила; предыдущая конфигурация сохранена"
                } else {
                    "Нет подтверждения правил от ядра. Отключитесь и подключитесь заново"
                }
                .into())
            }
        }
    }
    /// Apply without restarting Mihomo or changing the Windows proxy guard.
    pub fn apply_routing(
        &mut self,
        rules: &[String],
        policy: Policy,
    ) -> Result<RoutingChange, String> {
        let rules = multilink_core::normalize_direct_rules(rules)?;
        let old = multilink_core::normalize_direct_rules(&self.settings.rules)?;
        let changed: Vec<_> = old
            .iter()
            .filter(|r| !rules.contains(r))
            .chain(rules.iter().filter(|r| !old.contains(r)))
            .cloned()
            .collect();
        let connections = if changed.is_empty() {
            None
        } else {
            self.get("connections").ok()
        };
        let mut candidate = self.settings.clone();
        candidate.rules = rules;
        candidate.policy = policy;
        Self::ensure_rule_data(&candidate)?;
        self.apply_yaml(&config(&candidate, &self.secret)?)?;
        self.settings = candidate;
        let mut result = RoutingChange::default();
        if changed.iter().any(|rule| rule.starts_with("RULE-SET,")) {
            result.retry_tabs = true;
        }
        if let Some(connections) = connections.and_then(|v| v["connections"].as_array().cloned()) {
            let ip_changed = changed.iter().any(|r| r.starts_with("IP-CIDR"));
            for connection in connections {
                let host = connection["metadata"]["host"].as_str().unwrap_or_default();
                let ip = connection["metadata"]["destinationIP"]
                    .as_str()
                    .and_then(|s| s.parse().ok());
                let process = ["process", "processPath"]
                    .iter()
                    .find_map(|field| {
                        connection["metadata"][*field]
                            .as_str()
                            .filter(|s| !s.is_empty())
                    })
                    .and_then(|path| path.rsplit(['/', '\\']).next())
                    .unwrap_or_default();
                if changed.iter().any(|rule| {
                    multilink_core::exception_matches(rule, host, ip)
                        || rule
                            .strip_prefix("PROCESS-NAME,")
                            .and_then(|r| r.split(',').next())
                            .is_some_and(|name| name.eq_ignore_ascii_case(process))
                }) {
                    if let Some(id) = connection["id"].as_str() {
                        if self
                            .client
                            .delete(format!("http://127.0.0.1:{CONTROLLER}/connections/{id}"))
                            .bearer_auth(&self.secret)
                            .send()
                            .is_ok_and(|r| r.status().is_success())
                        {
                            result.closed_connections += 1;
                        } else {
                            result.retry_tabs = true;
                        }
                    }
                } else if ip_changed && ip.is_none() && !host.is_empty() {
                    result.retry_tabs = true;
                }
            }
        } else if !changed.is_empty() {
            result.retry_tabs = true;
        }
        Ok(result)
    }
    /// Apply updated settings (e.g. toggled nodes, policies) without restarting Mihomo.
    pub fn apply_settings(&mut self, settings: &Settings) -> Result<(), String> {
        let mut candidate = settings.clone();
        multilink_core::exclude_expired_access(&mut candidate);
        Self::ensure_rule_data(&candidate)?;
        self.apply_yaml(&config(&candidate, &self.secret)?)?;
        self.settings = candidate;
        Ok(())
    }
    pub fn metrics(&mut self) -> Result<Metrics, String> {
        if self
            .child
            .try_wait()
            .map_err(|_| "Ошибка процесса")?
            .is_some()
        {
            return Err("Процесс Mihomo завершился".into());
        }
        let data = self.get("connections")?;
        let download = data["downloadTotal"]
            .as_u64()
            .ok_or("Нет статистики downloadTotal")?;
        let upload = data["uploadTotal"]
            .as_u64()
            .ok_or("Нет статистики uploadTotal")?;
        let now = Instant::now();
        let (down_rate, up_rate) = self
            .last
            .map(|(time, d, u)| {
                let elapsed = now.duration_since(time).as_secs_f64().max(0.001);
                (
                    download.saturating_sub(d) as f64 / elapsed,
                    upload.saturating_sub(u) as f64 / elapsed,
                )
            })
            .unwrap_or_default();
        self.last = Some((now, download, upload));
        Ok(Metrics {
            download,
            upload,
            connections: data["connections"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            down_rate,
            up_rate,
        })
    }
    /// Verify actual Internet reachability through the local Mihomo proxy.
    pub fn tunnel_reachable(&self) -> bool {
        let Ok(proxy) = reqwest::Proxy::all(format!("http://127.0.0.1:{PORT}")) else {
            return false;
        };
        let Ok(client) = Client::builder()
            .no_proxy()
            .proxy(proxy)
            .timeout(Duration::from_secs(4))
            .build()
        else {
            return false;
        };
        client
            .get("https://www.gstatic.com/generate_204")
            .send()
            .is_ok_and(|response| response.status().is_success())
    }
    pub fn delays(
        &mut self,
    ) -> Result<Vec<(String, Option<u64>, multilink_core::NodeQuality)>, String> {
        let data = self.get("proxies")?;
        let proxies = data["proxies"].as_object().ok_or("Нет списка прокси")?;
        let mut effective = self.settings.clone();
        multilink_core::exclude_expired_access(&mut effective);
        let names = runtime_proxy_names(
            &effective
                .nodes
                .iter()
                .filter(|n| n.enabled)
                .collect::<Vec<_>>(),
        );
        let mut values = Vec::new();
        for node in &mut self.settings.nodes {
            if let Some(value) = names.get(&node.name).and_then(|name| proxies.get(name)) {
                let history = value["history"].as_array();
                let latest = history.and_then(|h| h.last());
                let delay = latest
                    .and_then(|v| v["delay"].as_u64())
                    .filter(|v| *v > 0 && value["alive"].as_bool() != Some(false));
                if node.enabled {
                    if let Some(time) = latest.and_then(|v| v["time"].as_str()) {
                        node.quality.observe(
                            &format!("{time}:{}", value["alive"].as_bool().unwrap_or(true)),
                            delay,
                        );
                    }
                }
                values.push((node.name.clone(), delay, node.quality.clone()));
            }
        }
        Ok(values)
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        if self.settings.tun && self.child.try_wait().ok().flatten().is_none() {
            let mut settings = self.settings.clone();
            settings.tun = false;
            if let Ok(contents) = config(&settings, &self.secret) {
                let _ = self.apply_yaml(&contents);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn fetch_subscription(url: &str) -> Result<String, String> {
    fetch_subscription_with_proxy(url, false)
}

pub fn fetch_subscription_with_proxy(url: &str, connected: bool) -> Result<String, String> {
    use std::io::Read;
    let url = multilink_core::subscription_url(url)?;
    let request = |proxy: bool| {
        let mut builder = Client::builder()
            .no_proxy()
            .user_agent("Mihomo")
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let url = attempt.url();
                if url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && attempt.previous().len() < 5
                {
                    attempt.follow()
                } else {
                    attempt.error("Unsafe or excessive subscription redirects")
                }
            }));
        if proxy {
            builder = builder.proxy(
                reqwest::Proxy::all(format!("http://127.0.0.1:{PORT}"))
                    .map_err(|_| "Не удалось настроить прокси для подписки".to_owned())?,
            );
        }
        let response = builder
            .build()
            .map_err(|_| "Ошибка HTTP-клиента".to_owned())?
            .get(&url)
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .map_err(|_| "Не удалось скачать подписку: проверьте адрес и доступность".to_owned())?;
        if !response.status().is_success() {
            return Err(format!(
                "Сервер подписки ответил HTTP {}",
                response.status().as_u16()
            ));
        }
        Ok(response)
    };
    let response =
        request(false).or_else(|error| if connected { request(true) } else { Err(error) })?;
    let mut bytes = Vec::new();
    response
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Ошибка чтения подписки")?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Подписка превышает 4 МБ".into());
    }
    String::from_utf8(bytes).map_err(|_| "Подписка не в UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tun_config_is_opt_in_and_accepted_by_installed_mihomo() {
        let mut settings = Settings::default();
        settings.nodes = multilink_core::parse_subscription(
            "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#Node",
        )
        .unwrap();
        let regular: serde_yaml::Value =
            serde_yaml::from_str(&config(&settings, "test").unwrap()).unwrap();
        assert!(regular.get("tun").is_none());
        settings.tun = true;
        let contents = config(&settings, "test").unwrap();
        let tun: serde_yaml::Value = serde_yaml::from_str(&contents).unwrap();
        assert_eq!(tun["tun"]["enable"], true);
        assert_eq!(tun["tun"]["device"], TUN_DEVICE);
        assert_eq!(tun["find-process-mode"], "strict");
        assert_eq!(tun["tun"]["dns-mode"], "disabled");
        let configurations = vec![contents];
        if binary().is_file() {
            let dir =
                std::env::temp_dir().join(format!("everty-tun-config-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&dir).unwrap();
            let path = dir.join("config.yaml");
            for contents in configurations {
                std::fs::write(&path, contents).unwrap();
                let output = Command::new(binary())
                    .args(["-t", "-d"])
                    .arg(&dir)
                    .arg("-f")
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    #[test]
    fn loopback_authenticated_no_direct_fallback() {
        let mut s = Settings::default();
        s.nodes = multilink_core::parse_subscription(
            "vless://00000000-0000-4000-8000-000000000001@example.com:443#Node",
        )
        .unwrap();
        s.rules
            .push(multilink_core::direct_rule("example.org").unwrap());
        let c: serde_yaml::Value =
            serde_yaml::from_str(&config(&s, "secret-test").unwrap()).unwrap();
        assert_eq!(c["external-controller"], "127.0.0.1:19090");
        assert_eq!(c["secret"], "secret-test");
        assert_eq!(c["rules"][0], "DOMAIN-SUFFIX,example.org,DIRECT");
        assert_eq!(c["rules"][1], "MATCH,MULTILINK");
        assert_eq!(c["proxy-groups"][0]["strategy"], "round-robin");
        s.nodes[0].enabled = false;
        assert!(config(&s, "x").is_err());
    }
    #[test]
    fn old_ip_rules_are_resolving_direct_rules_before_the_default_route() {
        let mut settings = Settings::default();
        settings.nodes = multilink_core::parse_subscription(
            "vless://00000000-0000-4000-8000-000000000001@example.invalid:443#Check",
        )
        .unwrap();
        settings.rules = vec!["IP-CIDR,127.0.0.1/32,DIRECT,no-resolve".into()];
        let c: serde_yaml::Value =
            serde_yaml::from_str(&config(&settings, "test").unwrap()).unwrap();
        assert_eq!(c["rules"][0], "IP-CIDR,127.0.0.1/32,DIRECT");
        assert_eq!(c["rules"][1], "MATCH,MULTILINK");
    }
    #[test]
    fn unavailable_country_routes_use_vpn_pool_and_keep_saved_binding() {
        let mut s = Settings::default();
        s.nodes=multilink_core::parse_subscription("vless://00000000-0000-4000-8000-000000000001@server.example:443#DE-01\nvless://00000000-0000-4000-8000-000000000002@server.example:444#US-01").unwrap();
        s.rules = vec!["DOMAIN-SUFFIX,example.com,COUNTRY-DE".into()];
        let c: serde_yaml::Value = serde_yaml::from_str(&config(&s, "test").unwrap()).unwrap();
        assert_eq!(c["proxy-groups"][1]["name"], "COUNTRY-DE");
        assert_eq!(c["proxy-groups"][1]["proxies"][0], "DE-01");
        assert_eq!(c["proxy-groups"].as_sequence().unwrap().len(), 2);
        s.nodes[0].enabled = false;
        let fallback: serde_yaml::Value =
            serde_yaml::from_str(&config(&s, "test").unwrap()).unwrap();
        assert_eq!(fallback["rules"][0], "DOMAIN-SUFFIX,example.com,MULTILINK");
        assert_eq!(
            multilink_core::unavailable_route_targets(&s),
            ["COUNTRY-DE"]
        );
        assert_eq!(s.rules[0], "DOMAIN-SUFFIX,example.com,COUNTRY-DE");
        s.rules = vec!["DOMAIN-SUFFIX,example.com,DE-01".into()];
        let fallback: serde_yaml::Value =
            serde_yaml::from_str(&config(&s, "test").unwrap()).unwrap();
        assert_eq!(fallback["rules"][0], "DOMAIN-SUFFIX,example.com,MULTILINK");
        assert_eq!(multilink_core::unavailable_route_targets(&s), ["DE-01"]);
        assert_eq!(s.rules[0], "DOMAIN-SUFFIX,example.com,DE-01");
        s.nodes[0].enabled = true;
        let restored: serde_yaml::Value =
            serde_yaml::from_str(&config(&s, "test").unwrap()).unwrap();
        assert_eq!(restored["rules"][0], "DOMAIN-SUFFIX,example.com,DE-01");
        s.nodes[0].name = "🇨🇿 Prague".into();
        s.rules = vec!["DOMAIN-SUFFIX,example.com,COUNTRY-CZ".into()];
        let c: serde_yaml::Value = serde_yaml::from_str(&config(&s, "test").unwrap()).unwrap();
        assert_eq!(c["proxy-groups"][1]["name"], "COUNTRY-CZ");
        assert_eq!(c["proxy-groups"][1]["proxies"][0], "🇨🇿 Prague");
    }

    /// End-to-end against the actual installed Mihomo. All target and VLESS
    /// traffic remains on loopback; no subscriptions or system proxy are used.
    #[test]
    #[ignore = "requires installed Mihomo and free application ports"]
    fn real_mihomo_routes_http_connect_and_live_exceptions() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        struct Server {
            port: u16,
            stop: Arc<AtomicBool>,
            thread: Option<std::thread::JoinHandle<()>>,
        }
        impl Drop for Server {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Relaxed);
                let _ = TcpStream::connect(("127.0.0.1", self.port));
                if let Some(t) = self.thread.take() {
                    let _ = t.join();
                }
            }
        }
        fn headers(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
            let mut data = Vec::new();
            let mut byte = [0];
            while !data.ends_with(b"\r\n\r\n") && data.len() < 8192 {
                stream.read_exact(&mut byte)?;
                data.push(byte[0]);
            }
            Ok(data)
        }
        fn server(vless: bool, body: &'static str) -> Server {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let thread = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(mut stream) = stream {
                        std::thread::spawn(move || {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                            let result = (|| -> std::io::Result<()> {
                                if vless {
                                    let mut initial = [0; 18];
                                    stream.read_exact(&mut initial)?;
                                    let mut options = vec![0; initial[17] as usize];
                                    stream.read_exact(&mut options)?;
                                    let mut request = [0; 4];
                                    stream.read_exact(&mut request)?;
                                    let length = match request[3] {
                                        1 => 4,
                                        2 => {
                                            let mut n = [0];
                                            stream.read_exact(&mut n)?;
                                            n[0] as usize
                                        }
                                        3 => 16,
                                        _ => return Ok(()),
                                    };
                                    let mut address = vec![0; length];
                                    stream.read_exact(&mut address)?;
                                    if u16::from_be_bytes([request[1], request[2]]) == 443 {
                                        return Ok(());
                                    } // skip the health probe
                                    stream.write_all(&[initial[0], 0])?;
                                }
                                headers(&mut stream)?;
                                write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())?;
                                Ok(())
                            })();
                            let _ = result;
                        });
                    }
                }
            });
            Server {
                port,
                stop,
                thread: Some(thread),
            }
        }
        fn connect(host: &str, port: u16) -> TcpStream {
            let mut stream = TcpStream::connect(("127.0.0.1", PORT)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(
                stream,
                "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n"
            )
            .unwrap();
            let reply = headers(&mut stream).unwrap();
            assert!(reply.starts_with(b"HTTP/1.1 200"), "CONNECT failed");
            stream
        }
        fn fetch(host: &str, port: u16, tunnel: bool) -> String {
            let mut stream = if tunnel {
                connect(host, port)
            } else {
                TcpStream::connect(("127.0.0.1", PORT)).unwrap()
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let uri = if tunnel {
                "/".into()
            } else {
                format!("http://{host}:{port}/")
            };
            write!(
                stream,
                "GET {uri} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).unwrap();
            reply.split("\r\n\r\n").nth(1).unwrap_or_default().into()
        }
        let target = server(false, "DIRECT");
        let upstream = server(true, "PROXY");
        let mut settings = Settings::default();
        settings.system_proxy = false;
        settings.nodes = multilink_core::parse_subscription(&format!(
            "vless://00000000-0000-4000-8000-000000000001@127.0.0.1:{}#Loopback-only",
            upstream.port
        ))
        .unwrap();
        let mut engine = Engine::start(&settings).unwrap();
        assert_eq!(fetch("localhost", target.port, false), "PROXY");
        assert_eq!(fetch("localhost", target.port, true), "PROXY");
        let mut held = connect("localhost", target.port);
        held.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if engine.get("connections").unwrap()["connections"]
                .as_array()
                .is_some_and(|items| items.iter().any(|c| c["metadata"]["host"] == "localhost"))
            {
                break;
            }
            assert!(Instant::now() < until, "held connection not visible");
            std::thread::sleep(Duration::from_millis(10));
        }
        let domain = multilink_core::direct_rule("http://localhost/a/path").unwrap();
        let applied = engine.apply_routing(&[domain], Policy::Balance).unwrap();
        assert!(
            applied.closed_connections >= 1,
            "affected existing tunnel was not retired"
        );
        let mut byte = [0];
        assert!(!matches!(held.read(&mut byte),Ok(n) if n>0));
        assert_eq!(fetch("localhost", target.port, false), "DIRECT");
        assert_eq!(fetch("localhost", target.port, true), "DIRECT");
        engine.apply_routing(&[], Policy::Balance).unwrap();
        assert_eq!(fetch("localhost", target.port, true), "PROXY");
        let ip = multilink_core::direct_rule("127.0.0.1").unwrap();
        engine.apply_routing(&[ip], Policy::Balance).unwrap();
        assert_eq!(
            fetch("localhost", target.port, false),
            "DIRECT",
            "IP exception must resolve domain targets"
        );
        assert_eq!(fetch("localhost", target.port, true), "DIRECT");
        assert_eq!(fetch("127.0.0.1", target.port, true), "DIRECT");
        // localhost is a built-in hosts entry, which Mihomo resolves before
        // rule matching. Use a loopback DNS responder for the no-resolve
        // regression so the test exercises real domain-to-IP resolution.
        struct Dns {
            stop: Arc<AtomicBool>,
            thread: Option<std::thread::JoinHandle<()>>,
        }
        impl Drop for Dns {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Relaxed);
                if let Some(t) = self.thread.take() {
                    let _ = t.join();
                }
            }
        }
        let socket = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let dns_port = socket.local_addr().unwrap().port();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while !stopped.load(Ordering::Relaxed) {
                let Ok((len, peer)) = socket.recv_from(&mut buffer) else {
                    continue;
                };
                if len < 12 {
                    continue;
                }
                let mut end = 12;
                while end < len && buffer[end] != 0 {
                    end += 1;
                }
                if end + 5 > len {
                    continue;
                }
                let is_a = buffer[end + 1..end + 3] == [0, 1];
                end += 5;
                let mut reply = vec![
                    buffer[0],
                    buffer[1],
                    0x81,
                    0x80,
                    0,
                    1,
                    0,
                    u8::from(is_a),
                    0,
                    0,
                    0,
                    0,
                ];
                reply.extend_from_slice(&buffer[12..end]);
                if is_a {
                    reply.extend_from_slice(&[
                        0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 1, 0, 4, 127, 0, 0, 1,
                    ]);
                }
                let _ = socket.send_to(&reply, peer);
            }
        });
        let _dns = Dns {
            stop,
            thread: Some(thread),
        };
        let mut legacy: serde_yaml::Value =
            serde_yaml::from_str(&config(&settings, &engine.secret).unwrap()).unwrap();
        let dns = serde_yaml::to_value(
            json!({"enable":true,"nameserver":[format!("udp://127.0.0.1:{dns_port}")]}),
        )
        .unwrap();
        legacy["dns"] = dns.clone();
        legacy["rules"] = serde_yaml::to_value(vec![
            "IP-CIDR,127.0.0.1/32,DIRECT,no-resolve",
            "MATCH,MULTILINK",
        ])
        .unwrap();
        engine
            .apply_yaml(&serde_yaml::to_string(&legacy).unwrap())
            .unwrap();
        assert_eq!(
            fetch("route-test.invalid", target.port, true),
            "PROXY",
            "reproduction: no-resolve skips the domain's IP exception"
        );
        settings.rules = vec![multilink_core::direct_rule("127.0.0.1").unwrap()];
        let mut fixed: serde_yaml::Value =
            serde_yaml::from_str(&config(&settings, &engine.secret).unwrap()).unwrap();
        fixed["dns"] = dns.clone();
        engine
            .apply_yaml(&serde_yaml::to_string(&fixed).unwrap())
            .unwrap();
        assert_eq!(
            fetch("route-test.invalid", target.port, true),
            "DIRECT",
            "IP exception must perform DNS resolution before matching"
        );
        settings.rules = vec!["RULE-SET,RU-SITES,DIRECT".into()];
        Engine::ensure_rule_data(&settings).unwrap();
        let mut ru: serde_yaml::Value =
            serde_yaml::from_str(&config(&settings, &engine.secret).unwrap()).unwrap();
        ru["dns"] = dns;
        engine
            .apply_yaml(&serde_yaml::to_string(&ru).unwrap())
            .unwrap();
        assert_eq!(
            fetch("ozon.com", target.port, true),
            "DIRECT",
            "RU rule set includes international domains"
        );
        assert_eq!(fetch("new-test-site.ru", target.port, true), "DIRECT");
        assert_eq!(fetch("xn--e1afmkfd.xn--p1ai", target.port, true), "DIRECT");
        assert_eq!(
            fetch("route-test.invalid", target.port, true),
            "PROXY",
            "unrelated domains remain proxied"
        );
        let germany = server(true, "GERMANY");
        settings.nodes.push(
            multilink_core::parse_subscription(&format!(
                "vless://00000000-0000-4000-8000-000000000002@127.0.0.1:{}#DE-01",
                germany.port
            ))
            .unwrap()
            .remove(0),
        );
        settings.rules.clear();
        engine.settings = settings.clone();
        engine
            .apply_routing(
                &["DOMAIN-SUFFIX,route-test.invalid,COUNTRY-DE".into()],
                Policy::Balance,
            )
            .unwrap();
        assert_eq!(
            fetch("route-test.invalid", target.port, true),
            "GERMANY",
            "country group must use that country's server"
        );
        engine
            .apply_routing(&["RULE-SET,RU-SITES,COUNTRY-DE".into()], Policy::Balance)
            .unwrap();
        assert_eq!(
            fetch("ozon.com", target.port, true),
            "GERMANY",
            "the RU category itself can target a country"
        );
        // Explicit per-site routing outranks the broad Russian set.
        engine
            .apply_routing(
                &[
                    "RULE-SET,RU-SITES,DIRECT".into(),
                    "DOMAIN-SUFFIX,ozon.com,COUNTRY-DE".into(),
                ],
                Policy::Balance,
            )
            .unwrap();
        assert_eq!(
            fetch("ozon.com", target.port, true),
            "GERMANY",
            "individual rules take priority over country-wide rules"
        );
        // Rejected configuration must leave the active route intact.
        assert!(engine.apply_yaml("proxies: [invalid").is_err());
        assert_eq!(fetch("ozon.com", target.port, true), "GERMANY");
        engine.metrics().unwrap();
    }
}
#[test]
fn server_and_country_with_same_name_are_distinct_routes() {
    let mut settings = Settings::default();
    settings.nodes = multilink_core::parse_subscription(
        "vless://00000000-0000-4000-8000-000000000001@one.invalid:443#COUNTRY-US\n\
             vless://00000000-0000-4000-8000-000000000002@two.invalid:443#US%2C%20New%20York",
    )
    .unwrap();
    settings.rules = vec![
        "DOMAIN-SUFFIX,group.example,COUNTRY-US".into(),
        format!(
            "PROCESS-NAME,antigravity.exe,{}",
            multilink_core::server_route_target("COUNTRY-US")
        ),
        format!(
            "DOMAIN-SUFFIX,other.example,{}",
            multilink_core::server_route_target("US, New York")
        ),
    ];
    assert!(multilink_core::unavailable_route_targets(&settings).is_empty());
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&config(&settings, "test").unwrap()).unwrap();
    assert_eq!(yaml["proxy-groups"][1]["name"], "COUNTRY-US");
    let nodes = yaml["proxies"].as_sequence().unwrap();
    let (one, two) = (
        nodes[0]["name"].as_str().unwrap(),
        nodes[1]["name"].as_str().unwrap(),
    );
    assert_ne!(one, "COUNTRY-US");
    assert_ne!(one, two);
    let rules = yaml["rules"].as_sequence().unwrap();
    assert!(rules.contains(&serde_yaml::Value::String(format!(
        "PROCESS-NAME,antigravity.exe,{one}"
    ))));
    assert!(rules.contains(&serde_yaml::Value::String(format!(
        "DOMAIN-SUFFIX,other.example,{two}"
    ))));
    assert!(rules.contains(&serde_yaml::Value::String(
        "DOMAIN-SUFFIX,group.example,COUNTRY-US".into()
    )));
}
