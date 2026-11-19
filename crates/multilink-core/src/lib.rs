use base64::{engine::general_purpose, Engine};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::collections::HashSet;
use url::Url;
mod subscriptions;
pub use subscriptions::*;
mod profile;
pub use profile::*;
mod access;
pub use access::*;
mod extra_import;
pub use extra_import::{is_lan_node, LAN_CIPHER, LAN_PORT};
pub mod antigravity_patch;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NodeQuality {
    pub samples: u32,
    pub latency_ms: f64,
    pub jitter_ms: f64,
    pub reliability: f64,
    pub last_sample: String,
    #[serde(default)]
    pub history: Vec<u16>,
}
impl NodeQuality {
    pub fn observe(&mut self, stamp: &str, delay: Option<u64>) {
        if stamp.is_empty() || self.last_sample == stamp {
            return;
        }
        self.last_sample = stamp.to_owned();
        let success = if delay.is_some() { 1.0 } else { 0.0 };
        if self.samples == 0 {
            self.reliability = success;
        } else {
            self.reliability = self.reliability * 0.75 + success * 0.25;
        }
        if let Some(delay) = delay {
            let delay_f = delay as f64;
            if self.latency_ms <= 0.0 {
                self.latency_ms = delay_f;
            } else {
                self.jitter_ms = self.jitter_ms * 0.75 + (delay_f - self.latency_ms).abs() * 0.25;
                self.latency_ms = self.latency_ms * 0.75 + delay_f * 0.25;
            }
            self.history.push(delay.min(9999) as u16);
        } else {
            self.history.push(0);
        }
        if self.history.len() > 10 {
            self.history.remove(0);
        }
        self.samples = self.samples.saturating_add(1);
    }
    pub fn score(&self) -> Option<u8> {
        if self.samples == 0 {
            return None;
        }
        let speed = 1000.0 / (1000.0 + self.latency_ms.max(1.0));
        let stability = 300.0 / (300.0 + self.jitter_ms.max(0.0));
        Some(
            (100.0 * self.reliability.clamp(0.0, 1.0) * (0.75 * speed + 0.25 * stability))
                .round()
                .clamp(0.0, 100.0) as u8,
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub enabled: bool,
    pub proxy: Mapping,
    /// Xray's optional fallback spider path. Mihomo does not expose this option;
    /// retain it as source metadata, never inject unknown fields into its config.
    #[serde(default)]
    pub spider_x: Option<String>,
    #[serde(skip)]
    pub delay: Option<u64>,
    #[serde(default)]
    pub quality: NodeQuality,
    #[serde(default)]
    pub subscription_ids: Vec<u64>,
    #[serde(default = "default_manual_node")]
    pub manually_added: bool,
}

impl Node {
    /// Android LAN sharing identifies an endpoint by server:port:auth.
    pub fn subscription_signature(&self) -> Option<String> {
        let server = self.proxy.get(Value::from("server"))?.as_str()?;
        let port = self.proxy.get(Value::from("port"))?.as_u64()?;
        let auth = self
            .proxy
            .get(Value::from("uuid"))
            .or_else(|| self.proxy.get(Value::from("password")))?
            .as_str()?;
        Some(format!("{server}:{port}:{auth}"))
    }
    pub fn to_vless_uri(&self) -> Option<String> {
        self.to_uri()
    }
    pub fn to_uri(&self) -> Option<String> {
        let proto = self.proxy.get(Value::from("type"))?.as_str()?;
        let server = self.proxy.get(Value::from("server"))?.as_str()?;
        let port = self.proxy.get(Value::from("port"))?.as_u64()?;
        let encoded_name =
            url::form_urlencoded::byte_serialize(self.name.as_bytes()).collect::<String>();
        match proto {
            "vless" => {
                let uuid = self.proxy.get(Value::from("uuid"))?.as_str()?;
                let mut query = Vec::new();
                if let Some(network) = self
                    .proxy
                    .get(Value::from("network"))
                    .and_then(Value::as_str)
                {
                    query.push(format!("type={network}"));
                }
                let tls = self
                    .proxy
                    .get(Value::from("tls"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let reality = self
                    .proxy
                    .get(Value::from("reality-opts"))
                    .and_then(Value::as_mapping);
                if let Some(ro) = reality {
                    query.push("security=reality".to_string());
                    if let Some(pbk) = ro.get(Value::from("public-key")).and_then(Value::as_str) {
                        query.push(format!("pbk={pbk}"));
                    }
                    if let Some(sid) = ro.get(Value::from("short-id")).and_then(Value::as_str) {
                        query.push(format!("sid={sid}"));
                    }
                    if let Some(spx) = &self.spider_x {
                        query.push(format!("spx={spx}"));
                    }
                } else if tls {
                    query.push("security=tls".to_string());
                }
                if let Some(sni) = self
                    .proxy
                    .get(Value::from("servername"))
                    .and_then(Value::as_str)
                {
                    query.push(format!("sni={sni}"));
                }
                if let Some(fp) = self
                    .proxy
                    .get(Value::from("client-fingerprint"))
                    .and_then(Value::as_str)
                {
                    query.push(format!("fp={fp}"));
                }
                if let Some(flow) = self.proxy.get(Value::from("flow")).and_then(Value::as_str) {
                    query.push(format!("flow={flow}"));
                }
                if let Some(ws) = self
                    .proxy
                    .get(Value::from("ws-opts"))
                    .and_then(Value::as_mapping)
                {
                    if let Some(path) = ws.get(Value::from("path")).and_then(Value::as_str) {
                        let encoded_path =
                            url::form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>();
                        query.push(format!("path={encoded_path}"));
                    }
                }
                if let Some(xh) = self
                    .proxy
                    .get(Value::from("xhttp-opts"))
                    .and_then(Value::as_mapping)
                {
                    if let Some(path) = xh.get(Value::from("path")).and_then(Value::as_str) {
                        let encoded_path =
                            url::form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>();
                        query.push(format!("path={encoded_path}"));
                    }
                }
                let q = if query.is_empty() {
                    String::new()
                } else {
                    format!("?{}", query.join("&"))
                };
                Some(format!("vless://{uuid}@{server}:{port}{q}#{encoded_name}"))
            }
            "hysteria2" => {
                let password = self
                    .proxy
                    .get(Value::from("password"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let mut query = Vec::new();
                if let Some(sni) = self.proxy.get(Value::from("sni")).and_then(Value::as_str) {
                    query.push(format!("sni={sni}"));
                }
                if let Some(obfs) = self.proxy.get(Value::from("obfs")).and_then(Value::as_str) {
                    query.push(format!("obfs={obfs}"));
                }
                if let Some(obfs_pw) = self
                    .proxy
                    .get(Value::from("obfs-password"))
                    .and_then(Value::as_str)
                {
                    query.push(format!("obfs-password={obfs_pw}"));
                }
                let q = if query.is_empty() {
                    String::new()
                } else {
                    format!("?{}", query.join("&"))
                };
                Some(format!("hysteria2://{password}@{server}:{port}{q}#{encoded_name}"))
            }
            "trojan" => {
                let password = self.proxy.get(Value::from("password")).and_then(Value::as_str)?;
                let mut query = Vec::new();
                if let Some(sni) = self.proxy.get(Value::from("sni")).and_then(Value::as_str) {
                    query.push(format!("sni={sni}"));
                }
                if let Some(net) = self.proxy.get(Value::from("network")).and_then(Value::as_str) {
                    query.push(format!("type={net}"));
                }
                let q = if query.is_empty() {
                    String::new()
                } else {
                    format!("?{}", query.join("&"))
                };
                Some(format!("trojan://{password}@{server}:{port}{q}#{encoded_name}"))
            }
            "ss" => {
                let cipher = self.proxy.get(Value::from("cipher")).and_then(Value::as_str)?;
                let password = self.proxy.get(Value::from("password")).and_then(Value::as_str)?;
                let user_info = general_purpose::STANDARD.encode(format!("{cipher}:{password}"));
                Some(format!("ss://{user_info}@{server}:{port}#{encoded_name}"))
            }
            "vmess" => {
                let uuid = self.proxy.get(Value::from("uuid")).and_then(Value::as_str)?;
                let net = self
                    .proxy
                    .get(Value::from("network"))
                    .and_then(Value::as_str)
                    .unwrap_or("tcp");
                let tls = if self
                    .proxy
                    .get(Value::from("tls"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "tls"
                } else {
                    ""
                };
                let sni = self
                    .proxy
                    .get(Value::from("servername"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let obj = serde_json::json!({
                    "v": "2",
                    "ps": self.name,
                    "add": server,
                    "port": port,
                    "id": uuid,
                    "net": net,
                    "tls": tls,
                    "sni": sni,
                });
                let encoded = general_purpose::STANDARD.encode(obj.to_string());
                Some(format!("vmess://{encoded}"))
            }
            _ => None,
        }
    }
}
fn default_manual_node() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Policy {
    #[default]
    Balance,
    Latency,
    Stable,
    Smart,
}
impl Policy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Balance => "Сбалансированный",
            Self::Latency => "Быстрый",
            Self::Stable => "Резервный",
            Self::Smart => "Умный выбор",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Balance => Self::Latency,
            Self::Latency => Self::Stable,
            Self::Stable => Self::Smart,
            Self::Smart => Self::Balance,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub lan_gateway: Option<String>,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub access: Option<ClientAccess>,
    #[serde(default)]
    pub pending_activation: Option<PendingActivation>,
    pub policy: Policy,
    // The old `system_proxy` field was an unused false placeholder. Use a new
    // persisted key so existing libraries get the new automatic default too.
    #[serde(rename = "automatic_system_proxy", default = "default_system_proxy")]
    pub system_proxy: bool,
    #[serde(default)]
    pub tun: bool,
    #[serde(default)]
    pub macos_network_migrated: bool,
    pub rules: Vec<String>,
    #[serde(default)]
    pub rule_notes: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_groups: Vec<Vec<String>>,
    pub nodes: Vec<Node>,
    pub imports: usize,
    #[serde(default)]
    pub rank_servers: bool,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub ru_defaults_initialized: bool,
    #[serde(default = "default_smart_tolerance")]
    pub smart_tolerance: u32,
    #[serde(default)]
    pub autoconnect: bool,
    #[serde(default)]
    pub rules_only: bool,
    #[serde(default)]
    pub pin_code: Option<String>,
    #[serde(default)]
    pub isolate_local_proxy: bool,
}
fn default_smart_tolerance() -> u32 {
    100
}
fn default_system_proxy() -> bool {
    !cfg!(target_os = "macos")
}
fn default_language() -> String {
    "system".into()
}
pub fn unavailable_route_targets(settings: &Settings) -> Vec<String> {
    let countries = available_countries(&settings.nodes);
    let mut missing = Vec::new();
    for rule in &settings.rules {
        if let Some(target) = rule.split(',').nth(2) {
            let available = matches!(
                target,
                "DIRECT"
                    | "MULTILINK"
                    | "MODE-SMART"
                    | "MODE-LATENCY"
                    | "MODE-STABLE"
                    | "MODE-BALANCE"
            ) || settings
                .nodes
                .iter()
                .any(|n| n.enabled && server_route_target(&n.name) == target)
                || target
                    .strip_prefix("COUNTRY-")
                    .is_some_and(|code| countries.iter().any(|(id, _)| id == code));
            if !available && !missing.iter().any(|v| v == target) {
                missing.push(target.to_owned());
            }
        }
    }
    missing
}

/// Distinguish country policies from server names and escape labels that
/// cannot appear verbatim in a comma-delimited routing rule.
pub fn server_route_target(name: &str) -> String {
    if name.is_empty()
        || name.len() > 128
        || name.trim() != name
        || name.starts_with("COUNTRY-")
        || name.starts_with("SERVER:")
        || matches!(name, "DIRECT" | "MULTILINK")
        || name.chars().any(|c| c.is_control() || c == ',')
    {
        format!(
            "SERVER:{}",
            url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>()
        )
    } else {
        name.to_owned()
    }
}

pub fn server_route_name(target: &str) -> Option<String> {
    let encoded = target.strip_prefix("SERVER:")?;
    let name = url::form_urlencoded::parse(format!("name={encoded}").as_bytes())
        .next()?
        .1
        .into_owned();
    (server_route_target(&name) == target).then_some(name)
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            lan_gateway: None,
            language: default_language(),
            access: None,
            pending_activation: None,
            policy: Policy::default(),
            system_proxy: default_system_proxy(),
            tun: false,
            macos_network_migrated: false,
            rules: Vec::new(),
            rule_notes: Default::default(),
            rule_groups: Vec::new(),
            nodes: Vec::new(),
            imports: 0,
            rank_servers: false,
            subscriptions: Vec::new(),
            ru_defaults_initialized: false,
            smart_tolerance: default_smart_tolerance(),
            autoconnect: false,
            rules_only: false,
            pin_code: None,
            isolate_local_proxy: false,
        }
    }
}
/// Portable routing data. Deliberately excludes subscriptions, node credentials and API tokens.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    pub version: u32,
    pub policy: Policy,
    pub rules: Vec<String>,
    #[serde(default)]
    pub notes: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Vec<String>>,
}
pub fn rule_key(rule: &str) -> String {
    rule.split(',').take(2).collect::<Vec<_>>().join(",")
}

/// Editor groups reference existing clauses with one shared routing target.
pub fn retain_rule_groups(groups: &mut Vec<Vec<String>>, rules: &[String]) {
    let targets: std::collections::HashMap<_, _> = rules
        .iter()
        .filter(|rule| !rule.starts_with("RULE-SET,"))
        .filter_map(|rule| rule.rsplit_once(','))
        .collect();
    let mut seen = HashSet::new();
    groups.retain_mut(|group| {
        let Some(target) = group.iter().find_map(|key| targets.get(key.as_str())) else {
            return false;
        };
        group.retain(|key| targets.get(key.as_str()) == Some(target) && seen.insert(key.clone()));
        !group.is_empty()
    });
}
impl RoutingConfig {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            version: 1,
            policy: settings.policy,
            rules: settings.rules.clone(),
            notes: settings.rule_notes.clone(),
            groups: settings.rule_groups.clone(),
        }
    }
    pub fn encode(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|_| "Не удалось создать файл маршрутов".into())
    }
    pub fn decode(text: &str) -> Result<Self, String> {
        if text.len() > 2 * 1024 * 1024 {
            return Err("Файл маршрутов превышает 2 МБ".into());
        }
        let mut value: Self = serde_json::from_str(text)
            .map_err(|_| "Ожидается JSON-конфиг маршрутов MultiLink".to_owned())?;
        if value.version != 1 {
            return Err("Неподдерживаемая версия конфига маршрутов".into());
        }
        if value.rules.len() > 5000 {
            return Err("В конфиге слишком много правил".into());
        }
        value.rules = normalize_direct_rules(&value.rules)?;
        if value
            .notes
            .values()
            .any(|note| note.len() > 4096 || note.contains('\0'))
        {
            return Err("Заметка превышает 4096 байт или содержит недопустимые символы".into());
        }
        value
            .notes
            .retain(|key, _| value.rules.iter().any(|rule| rule_key(rule) == *key));
        retain_rule_groups(&mut value.groups, &value.rules);
        Ok(value)
    }
    pub fn merged(&self, settings: &Settings) -> Self {
        let mut rules = settings.rules.clone();
        let mut notes = settings.rule_notes.clone();
        let mut groups = self.groups.clone();
        for rule in &self.rules {
            let key = rule_key(rule);
            rules.retain(|old| rule_key(old) != key);
            rules.push(rule.clone());
            notes.remove(&key);
            if let Some(note) = self.notes.get(&key) {
                notes.insert(key, note.clone());
            }
        }
        let imported: HashSet<_> = self.rules.iter().map(|rule| rule_key(rule)).collect();
        groups.extend(settings.rule_groups.iter().map(|group| {
            group
                .iter()
                .filter(|key| !imported.contains(*key))
                .cloned()
                .collect()
        }));
        retain_rule_groups(&mut groups, &rules);
        Self {
            version: 1,
            rules,
            notes,
            groups,
            policy: self.policy,
        }
    }
}

/// Parse the entire payload before committing anything. Unknown URI parameters
/// are errors, rather than silently changing transport or security semantics.
pub fn parse_subscription(input: &str) -> Result<Vec<Node>, String> {
    let input = input.trim().trim_start_matches('\u{feff}').trim();
    if input.len() > 4 * 1024 * 1024 {
        return Err("Подписка превышает 4 МБ".into());
    }
    if input.starts_with("everty-lan://") {
        return extra_import::lan_node(input);
    }
    if input
        .lines()
        .take(20)
        .any(|line| line.trim() == "[Interface]")
    {
        return extra_import::amnezia_conf(input);
    }
    if input.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(input) {
            if value.get("format").is_some() {
                return ProfileDocument::decode(input)?.into_nodes();
            }
            if value.get("outbounds").is_some() {
                return extra_import::xray_vless(&value);
            }
        }
    }
    if is_node_uri(input) {
        return parse_lines(input);
    }
    if let Ok(value) = serde_yaml::from_str::<Value>(input) {
        if let Some(items) = value.get("proxies").and_then(Value::as_sequence) {
            let mut nodes = Vec::new();
            for item in items {
                let mut proxy = item.as_mapping().cloned().ok_or("Некорректный узел YAML")?;
                let proto = proxy.get("type").and_then(Value::as_str).unwrap_or("");
                if !["vless", "hysteria2", "trojan", "ss", "vmess"].contains(&proto) {
                    return Err(format!("Неподдерживаемый протокол YAML: {proto}"));
                }
                let name = proxy
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("У узла нет имени")?
                    .to_owned();
                let server = proxy
                    .get("server")
                    .and_then(Value::as_str)
                    .ok_or("У узла нет адреса")?;
                if server.is_empty() {
                    return Err("Пустой адрес сервера".into());
                }
                let port = proxy
                    .get("port")
                    .and_then(Value::as_u64)
                    .ok_or("Некорректный порт")?;
                if !(1..=65535).contains(&port) {
                    return Err("Некорректный порт".into());
                }
                if ["vless", "vmess"].contains(&proto) {
                    if let Some(u) = proxy.get("uuid").and_then(Value::as_str) {
                        uuid::Uuid::parse_str(u).map_err(|_| "Некорректный UUID")?;
                    }
                }
                proxy.remove(Value::from("name"));
                nodes.push(Node {
                    name,
                    enabled: true,
                    proxy,
                    spider_x: None,
                    delay: None,
                    quality: NodeQuality::default(),
                    subscription_ids: Vec::new(),
                    manually_added: true,
                });
            }
            return finish(nodes);
        }
    }
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    for engine in [
        &general_purpose::STANDARD,
        &general_purpose::STANDARD_NO_PAD,
        &general_purpose::URL_SAFE,
        &general_purpose::URL_SAFE_NO_PAD,
    ] {
        if let Ok(bytes) = engine.decode(&compact) {
            if let Ok(text) = String::from_utf8(bytes) {
                if text.lines().any(is_node_uri) {
                    return parse_lines(&text);
                }
            }
        }
    }
    Err("Ожидается URI узла (VLESS/Hysteria2/Trojan/SS/VMess), Base64-список или Mihomo YAML".into())
}

pub fn is_node_uri(input: &str) -> bool {
    let input = input.trim();
    input.starts_with("vless://")
        || input.starts_with("hysteria2://")
        || input.starts_with("hy2://")
        || input.starts_with("trojan://")
        || input.starts_with("ss://")
        || input.starts_with("vmess://")
}

fn parse_lines(input: &str) -> Result<Vec<Node>, String> {
    input
        .split_whitespace()
        .map(parse_uri)
        .collect::<Result<Vec<_>, _>>()
        .and_then(finish)
}
fn finish(nodes: Vec<Node>) -> Result<Vec<Node>, String> {
    if nodes.is_empty() {
        Err("В подписке нет серверов".into())
    } else if nodes.len() > 2000 {
        Err("Лимит — 2000 серверов".into())
    } else {
        Ok(nodes)
    }
}

fn put(map: &mut Mapping, key: &str, value: impl Into<Value>) {
    map.insert(Value::from(key), value.into());
}

pub fn parse_uri(input: &str) -> Result<Node, String> {
    let input = input.trim();
    if input.starts_with("vless://") {
        parse_vless_uri(input)
    } else if input.starts_with("hysteria2://") || input.starts_with("hy2://") {
        parse_hysteria2(input)
    } else if input.starts_with("trojan://") {
        parse_trojan(input)
    } else if input.starts_with("ss://") {
        parse_ss(input)
    } else if input.starts_with("vmess://") {
        parse_vmess(input)
    } else {
        Err("Поддерживаются протоколы: VLESS, Hysteria2, Trojan, Shadowsocks, VMess".into())
    }
}

fn parse_vless_uri(input: &str) -> Result<Node, String> {
    let uri = Url::parse(input).map_err(|_| "Некорректная VLESS-ссылка")?;
    if uri.scheme() != "vless" {
        return Err("Поддерживаются только vless:// ссылки".into());
    }
    uuid::Uuid::parse_str(uri.username()).map_err(|_| "Некорректный UUID")?;
    let host = uri
        .host_str()
        .ok_or("Не указан сервер")?
        .trim_matches(['[', ']']);
    let port = uri.port().ok_or("Не указан порт")?;
    if port == 0 {
        return Err("Порт не может быть нулевым".into());
    }
    let mut params = std::collections::HashMap::new();
    if uri
        .query_pairs()
        .any(|(key, value)| key == "type" && !["", "tcp", "ws", "grpc", "xhttp", "h2", "http"].contains(&value.as_ref()))
    {
        return Err("Поддерживаются TCP, WebSocket, gRPC, XHTTP и H2; транспорт узла несовместим".into());
    }
    for (key, value) in uri.query_pairs() {
        if key == "fm" {
            return Err(
                "Фрагментация fm не поддерживается Mihomo; узел пропущен без изменения параметров"
                    .into(),
            );
        }
        if ![
            "type",
            "security",
            "sni",
            "fp",
            "pbk",
            "sid",
            "flow",
            "path",
            "host",
            "serviceName",
            "encryption",
            "allowInsecure",
            "alpn",
            "spx",
            "support-x25519mlkem768",
            "mode",
            "extra",
        ]
        .contains(&key.as_ref())
        {
            return Err(format!("Неподдерживаемый параметр: {key}"));
        }
        if params
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err("Повторяющийся параметр URI".into());
        }
    }
    let get = |key: &str| params.get(key).map(String::as_str).unwrap_or("");
    let mut p = Mapping::new();
    put(&mut p, "type", "vless");
    put(&mut p, "server", host);
    put(&mut p, "port", port as u64);
    put(&mut p, "uuid", uri.username());
    put(&mut p, "udp", true);
    match get("security") {
        "" | "none" => {}
        "tls" | "reality" => {
            put(&mut p, "tls", true);
            if !get("sni").is_empty() {
                put(&mut p, "servername", get("sni"));
            }
            if !get("fp").is_empty() {
                put(&mut p, "client-fingerprint", get("fp"));
            }
            if get("security") == "reality" {
                if get("pbk").is_empty() {
                    return Err("REALITY требует public key".into());
                }
                let mut opts = Mapping::new();
                put(&mut opts, "public-key", get("pbk"));
                put(&mut opts, "short-id", get("sid"));
                if !get("support-x25519mlkem768").is_empty() {
                    let supported = match get("support-x25519mlkem768") {
                        "true" | "1" => true,
                        "false" | "0" => false,
                        _ => return Err("Некорректный support-x25519mlkem768".into()),
                    };
                    put(&mut opts, "support-x25519mlkem768", supported);
                }
                put(&mut p, "reality-opts", Value::Mapping(opts));
            }
        }
        _ => return Err("Неподдерживаемый security".into()),
    }
    if !get("flow").is_empty() {
        put(&mut p, "flow", get("flow"));
    }
    if !get("allowInsecure").is_empty() {
        if !["0", "1", "true", "false"].contains(&get("allowInsecure")) {
            return Err("Некорректный allowInsecure".into());
        }
        put(
            &mut p,
            "skip-cert-verify",
            matches!(get("allowInsecure"), "1" | "true"),
        );
    }
    if !get("alpn").is_empty() {
        put(
            &mut p,
            "alpn",
            Value::Sequence(get("alpn").split(',').map(Value::from).collect()),
        );
    }
    match get("type") {
        "" | "tcp" => {}
        "ws" => {
            put(&mut p, "network", "ws");
            let mut opts = Mapping::new();
            put(
                &mut opts,
                "path",
                if get("path").is_empty() {
                    "/"
                } else {
                    get("path")
                },
            );
            if !get("host").is_empty() {
                let mut headers = Mapping::new();
                put(&mut headers, "Host", get("host"));
                put(&mut opts, "headers", Value::Mapping(headers));
            }
            put(&mut p, "ws-opts", Value::Mapping(opts));
        }
        "grpc" => {
            put(&mut p, "network", "grpc");
            let mut opts = Mapping::new();
            put(&mut opts, "grpc-service-name", get("serviceName"));
            put(&mut p, "grpc-opts", Value::Mapping(opts));
        }
        "xhttp" => {
            put(&mut p, "network", "xhttp");
            let mut opts = Mapping::new();
            put(
                &mut opts,
                "path",
                if get("path").is_empty() {
                    "/"
                } else {
                    get("path")
                },
            );
            if !get("host").is_empty() {
                let mut headers = Mapping::new();
                put(&mut headers, "Host", get("host"));
                put(&mut opts, "headers", Value::Mapping(headers));
            }
            if !get("mode").is_empty() {
                put(&mut opts, "mode", get("mode"));
            }
            if !get("extra").is_empty() {
                put(&mut opts, "extra", get("extra"));
            }
            put(&mut p, "xhttp-opts", Value::Mapping(opts));
        }
        "h2" | "http" => {
            put(&mut p, "network", "h2");
            let mut opts = Mapping::new();
            put(
                &mut opts,
                "path",
                if get("path").is_empty() {
                    "/"
                } else {
                    get("path")
                },
            );
            if !get("host").is_empty() {
                put(&mut opts, "host", Value::Sequence(vec![Value::from(get("host"))]));
            }
            put(&mut p, "h2-opts", Value::Mapping(opts));
        }
        _ => return Err("Поддерживаются TCP, WebSocket, gRPC, XHTTP и H2".into()),
    }
    let name = uri
        .fragment()
        .filter(|v| !v.is_empty())
        .map(|v| percent_decode(v))
        .unwrap_or_else(|| host.to_owned());
    Ok(Node {
        name,
        enabled: true,
        proxy: p,
        spider_x: params.get("spx").cloned(),
        delay: None,
        quality: NodeQuality::default(),
        subscription_ids: Vec::new(),
        manually_added: true,
    })
}

fn parse_hysteria2(input: &str) -> Result<Node, String> {
    let uri = Url::parse(input).map_err(|_| "Некорректная Hysteria2-ссылка")?;
    let host = uri
        .host_str()
        .ok_or("Не указан сервер")?
        .trim_matches(['[', ']']);
    let port = uri.port().unwrap_or(443);
    if port == 0 {
        return Err("Порт не может быть нулевым".into());
    }
    let mut p = Mapping::new();
    put(&mut p, "type", "hysteria2");
    put(&mut p, "server", host);
    put(&mut p, "port", port as u64);
    if !uri.username().is_empty() {
        put(&mut p, "password", uri.username());
    }
    let mut params = std::collections::HashMap::new();
    for (key, value) in uri.query_pairs() {
        params.insert(key.into_owned(), value.into_owned());
    }
    let get = |key: &str| params.get(key).map(String::as_str).unwrap_or("");
    if !get("sni").is_empty() {
        put(&mut p, "sni", get("sni"));
    }
    if !get("obfs").is_empty() {
        put(&mut p, "obfs", get("obfs"));
        if !get("obfs-password").is_empty() {
            put(&mut p, "obfs-password", get("obfs-password"));
        }
    }
    if !get("alpn").is_empty() {
        put(
            &mut p,
            "alpn",
            Value::Sequence(get("alpn").split(',').map(Value::from).collect()),
        );
    }
    if matches!(get("insecure"), "1" | "true") || matches!(get("allowInsecure"), "1" | "true") {
        put(&mut p, "skip-cert-verify", true);
    }
    let name = uri
        .fragment()
        .filter(|v| !v.is_empty())
        .map(percent_decode)
        .unwrap_or_else(|| host.to_owned());
    Ok(Node {
        name,
        enabled: true,
        proxy: p,
        spider_x: None,
        delay: None,
        quality: NodeQuality::default(),
        subscription_ids: Vec::new(),
        manually_added: true,
    })
}

fn parse_trojan(input: &str) -> Result<Node, String> {
    let uri = Url::parse(input).map_err(|_| "Некорректная Trojan-ссылка")?;
    let host = uri
        .host_str()
        .ok_or("Не указан сервер")?
        .trim_matches(['[', ']']);
    let port = uri.port().unwrap_or(443);
    if port == 0 {
        return Err("Порт не может быть нулевым".into());
    }
    let password = uri.username();
    if password.is_empty() {
        return Err("Нет пароля Trojan".into());
    }
    let mut p = Mapping::new();
    put(&mut p, "type", "trojan");
    put(&mut p, "server", host);
    put(&mut p, "port", port as u64);
    put(&mut p, "password", password);
    put(&mut p, "udp", true);
    let mut params = std::collections::HashMap::new();
    for (key, value) in uri.query_pairs() {
        params.insert(key.into_owned(), value.into_owned());
    }
    let get = |key: &str| params.get(key).map(String::as_str).unwrap_or("");
    if !get("sni").is_empty() {
        put(&mut p, "sni", get("sni"));
    }
    if matches!(get("allowInsecure"), "1" | "true") || matches!(get("insecure"), "1" | "true") {
        put(&mut p, "skip-cert-verify", true);
    }
    if !get("alpn").is_empty() {
        put(
            &mut p,
            "alpn",
            Value::Sequence(get("alpn").split(',').map(Value::from).collect()),
        );
    }
    match get("type") {
        "ws" => {
            put(&mut p, "network", "ws");
            let mut opts = Mapping::new();
            put(
                &mut opts,
                "path",
                if get("path").is_empty() {
                    "/"
                } else {
                    get("path")
                },
            );
            if !get("host").is_empty() {
                let mut headers = Mapping::new();
                put(&mut headers, "Host", get("host"));
                put(&mut opts, "headers", Value::Mapping(headers));
            }
            put(&mut p, "ws-opts", Value::Mapping(opts));
        }
        "grpc" => {
            put(&mut p, "network", "grpc");
            let mut opts = Mapping::new();
            put(&mut opts, "grpc-service-name", get("serviceName"));
            put(&mut p, "grpc-opts", Value::Mapping(opts));
        }
        _ => {}
    }
    let name = uri
        .fragment()
        .filter(|v| !v.is_empty())
        .map(percent_decode)
        .unwrap_or_else(|| host.to_owned());
    Ok(Node {
        name,
        enabled: true,
        proxy: p,
        spider_x: None,
        delay: None,
        quality: NodeQuality::default(),
        subscription_ids: Vec::new(),
        manually_added: true,
    })
}

fn parse_ss(input: &str) -> Result<Node, String> {
    let raw = input.trim_start_matches("ss://");
    let (body, fragment) = match raw.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (raw, None),
    };
    let name = fragment.map(percent_decode).unwrap_or_default();
    let (user_info, host_port) = if let Some((u, hp)) = body.split_once('@') {
        let decoded_user = decode_base64_str(u).unwrap_or_else(|| u.to_string());
        (decoded_user, hp.to_string())
    } else {
        let decoded = decode_base64_str(body).ok_or("Некорректная ссылка Shadowsocks")?;
        let (u, hp) = decoded.split_once('@').ok_or("Некорректная ссылка Shadowsocks")?;
        (u.to_string(), hp.to_string())
    };
    let (cipher, password) = user_info
        .split_once(':')
        .ok_or("Нет шифра/пароля Shadowsocks")?;
    let (host, port_str) = host_port
        .rsplit_once(':')
        .ok_or("Не указан порт Shadowsocks")?;
    let host = host.trim_matches(['[', ']']);
    let port: u16 = port_str
        .split('?')
        .next()
        .unwrap_or(port_str)
        .parse()
        .map_err(|_| "Некорректный порт")?;
    if port == 0 {
        return Err("Порт не может быть нулевым".into());
    }
    let mut p = Mapping::new();
    put(&mut p, "type", "ss");
    put(&mut p, "server", host);
    put(&mut p, "port", port as u64);
    put(&mut p, "cipher", cipher);
    put(&mut p, "password", password);
    put(&mut p, "udp", true);
    let node_name = if name.is_empty() {
        host.to_owned()
    } else {
        name
    };
    Ok(Node {
        name: node_name,
        enabled: true,
        proxy: p,
        spider_x: None,
        delay: None,
        quality: NodeQuality::default(),
        subscription_ids: Vec::new(),
        manually_added: true,
    })
}

fn parse_vmess(input: &str) -> Result<Node, String> {
    let b64 = input.trim_start_matches("vmess://").trim();
    let decoded = decode_base64_str(b64).ok_or("Не удалось декодировать VMess-ссылку")?;
    let v: serde_json::Value =
        serde_json::from_str(&decoded).map_err(|_| "Некорректный JSON VMess")?;
    let host = v
        .get("add")
        .and_then(|x| x.as_str())
        .ok_or("Не указан адрес сервера")?;
    let port = match v.get("port") {
        Some(serde_json::Value::Number(n)) => n.as_u64().ok_or("Некорректный порт")? as u16,
        Some(serde_json::Value::String(s)) => s.parse::<u16>().map_err(|_| "Некорректный порт")?,
        _ => return Err("Не указан порт".into()),
    };
    if port == 0 {
        return Err("Порт не может быть нулевым".into());
    }
    let uuid_str = v
        .get("id")
        .and_then(|x| x.as_str())
        .ok_or("Не указан UUID VMess")?;
    uuid::Uuid::parse_str(uuid_str).map_err(|_| "Некорректный UUID VMess")?;
    let alter_id = match v.get("aid") {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse::<u64>().unwrap_or(0),
        _ => 0,
    };
    let net = v.get("net").and_then(|x| x.as_str()).unwrap_or("tcp");
    let tls = v.get("tls").and_then(|x| x.as_str()).unwrap_or("");
    let sni = v
        .get("sni")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("host").and_then(|x| x.as_str()))
        .unwrap_or("");
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("");
    let name = v.get("ps").and_then(|x| x.as_str()).unwrap_or(host);

    let mut p = Mapping::new();
    put(&mut p, "type", "vmess");
    put(&mut p, "server", host);
    put(&mut p, "port", port as u64);
    put(&mut p, "uuid", uuid_str);
    put(&mut p, "alterId", alter_id);
    put(&mut p, "cipher", "auto");
    put(&mut p, "udp", true);
    if tls == "tls" {
        put(&mut p, "tls", true);
        if !sni.is_empty() {
            put(&mut p, "servername", sni);
        }
    }
    match net {
        "ws" => {
            put(&mut p, "network", "ws");
            let mut opts = Mapping::new();
            put(
                &mut opts,
                "path",
                if path.is_empty() { "/" } else { path },
            );
            let h = v.get("host").and_then(|x| x.as_str()).unwrap_or("");
            if !h.is_empty() {
                let mut headers = Mapping::new();
                put(&mut headers, "Host", h);
                put(&mut opts, "headers", Value::Mapping(headers));
            }
            put(&mut p, "ws-opts", Value::Mapping(opts));
        }
        "grpc" => {
            put(&mut p, "network", "grpc");
            let mut opts = Mapping::new();
            put(&mut opts, "grpc-service-name", path);
            put(&mut p, "grpc-opts", Value::Mapping(opts));
        }
        _ => {}
    }
    Ok(Node {
        name: name.to_owned(),
        enabled: true,
        proxy: p,
        spider_x: None,
        delay: None,
        quality: NodeQuality::default(),
        subscription_ids: Vec::new(),
        manually_added: true,
    })
}

fn decode_base64_str(s: &str) -> Option<String> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    for engine in [
        &general_purpose::STANDARD,
        &general_purpose::STANDARD_NO_PAD,
        &general_purpose::URL_SAFE,
        &general_purpose::URL_SAFE_NO_PAD,
    ] {
        if let Ok(bytes) = engine.decode(&clean) {
            if let Ok(res) = String::from_utf8(bytes) {
                return Some(res);
            }
        }
    }
    None
}

pub struct ImportReport {
    pub nodes: Vec<Node>,
    /// Only positions and static parser errors: never subscription credentials.
    pub skipped: Vec<String>,
    /// Explicit compatibility changes applied to accepted nodes.
    pub warnings: Vec<String>,
    pub expires_at: Option<u64>,
}

/// Mixed subscriptions can contain transports belonging to another engine.
/// Keep usable nodes, and explicitly report every rejected entry to the UI.
pub fn import_subscription(input: &str) -> Result<ImportReport, String> {
    match parse_subscription(input) {
        Ok(nodes) => {
            return Ok(ImportReport {
                nodes,
                skipped: Vec::new(),
                warnings: Vec::new(),
                expires_at: None,
            })
        }
        Err(error) if input.len() > 4 * 1024 * 1024 => return Err(error),
        Err(_) => {}
    }
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let decoded = if is_node_uri(input.trim()) {
        input.to_owned()
    } else {
        let mut decoded = None;
        for engine in [
            &general_purpose::STANDARD,
            &general_purpose::STANDARD_NO_PAD,
            &general_purpose::URL_SAFE,
            &general_purpose::URL_SAFE_NO_PAD,
        ] {
            if let Ok(bytes) = engine.decode(&compact) {
                if let Ok(s) = String::from_utf8(bytes) {
                    if s.lines().any(is_node_uri) {
                        decoded = Some(s);
                        break;
                    }
                }
            }
        }
        match decoded {
            Some(s) => s,
            _ => {
                return parse_subscription(input).map(|nodes| ImportReport {
                    nodes,
                    skipped: Vec::new(),
                    warnings: Vec::new(),
                    expires_at: None,
                })
            }
        }
    };
    let lines: Vec<_> = decoded.split_whitespace().collect();
    if lines.len() > 2000 {
        return Err("Лимит — 2000 серверов".into());
    }
    let mut report = ImportReport {
        nodes: Vec::new(),
        skipped: Vec::new(),
        warnings: Vec::new(),
        expires_at: None,
    };
    for (i, line) in lines.iter().enumerate() {
        let converted = without_fm(line);
        match parse_uri(converted.as_deref().unwrap_or(line)) {
            Ok(node) => {
                report.nodes.push(node);
                if converted.is_some() {
                    report
                        .warnings
                        .push(format!("Узел {}: импортирован без фрагментации FM", i + 1));
                }
            }
            Err(error) => report.skipped.push(format!("Узел {}: {error}", i + 1)),
        }
    }
    if report.nodes.is_empty() {
        return Err(format!(
            "Нет совместимых узлов. {}",
            report
                .skipped
                .first()
                .map(String::as_str)
                .unwrap_or("Пустая подписка")
        ));
    }
    Ok(report)
}

// Only the user-authorized FM setting is omitted. Unknown transports, TLS
// options and all other unsupported settings still go through strict validation.
fn without_fm(input: &str) -> Option<String> {
    let mut uri = Url::parse(input).ok()?;
    if uri.scheme() != "vless" || !uri.query_pairs().any(|(key, _)| key == "fm") {
        return None;
    }
    let pairs: Vec<_> = uri
        .query_pairs()
        .filter(|(key, _)| key != "fm")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    uri.set_query(None);
    if !pairs.is_empty() {
        uri.query_pairs_mut().extend_pairs(pairs);
    }
    Some(uri.into())
}
fn percent_decode(s: &str) -> String {
    // URL form decoding handles UTF-8 names without exposing credentials.
    url::form_urlencoded::parse(format!("n={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

pub fn merge_nodes(existing: &mut Vec<Node>, new: Vec<Node>) -> usize {
    let mut fingerprints: HashSet<String> = existing
        .iter()
        .filter_map(|n| serde_json::to_string(&n.proxy).ok())
        .collect();
    let before = existing.len();
    for mut node in new {
        if let Some(known) = existing.iter_mut().find(|known| known.proxy == node.proxy) {
            known.manually_added = true;
            continue;
        }
        if fingerprints.insert(serde_json::to_string(&node.proxy).unwrap_or_default()) {
            let base = node.name.clone();
            let mut i = 2;
            while node.name == "MULTILINK" || existing.iter().any(|n| n.name == node.name) {
                node.name = format!("{base} ({i})");
                i += 1;
            }
            existing.push(node);
        }
    }
    existing.len() - before
}

/// Returns a validated Mihomo DIRECT rule. No raw YAML or rule injection.
pub fn direct_rule(input: &str) -> Result<String, String> {
    let mut input = input.trim().to_lowercase();
    if input.chars().any(|c| c.is_control() || c == ',') {
        return Err("Введите домен, URL, IP или CIDR без управляющих символов".into());
    }
    // Copying a page URL should exclude its host, not require hand editing.
    if input.contains("://")
        || (input.contains('/') && input.parse::<ipnet::IpNet>().is_err())
        || input.contains('?')
        || input.contains('#')
        || (input.contains(':')
            && input.parse::<std::net::IpAddr>().is_err()
            && input.parse::<ipnet::IpNet>().is_err())
    {
        let text = if input.contains("://") {
            input.clone()
        } else {
            format!("https://{input}")
        };
        let url = Url::parse(&text).map_err(|_| "Некорректный адрес исключения")?;
        if !["http", "https"].contains(&url.scheme()) {
            return Err("Вставьте HTTP/HTTPS URL или домен".into());
        }
        input = url
            .host_str()
            .ok_or("В адресе нет домена")?
            .trim_matches(['[', ']'])
            .to_owned();
    }
    if let Ok(ip) = input.parse::<std::net::IpAddr>() {
        return Ok(format!(
            "{},{}/{},DIRECT",
            if ip.is_ipv4() { "IP-CIDR" } else { "IP-CIDR6" },
            ip,
            if ip.is_ipv4() { 32 } else { 128 }
        ));
    }
    if let Ok(net) = input.parse::<ipnet::IpNet>() {
        return Ok(format!(
            "{},{net},DIRECT",
            if net.addr().is_ipv4() {
                "IP-CIDR"
            } else {
                "IP-CIDR6"
            }
        ));
    }
    let domain = input
        .strip_prefix("*.")
        .unwrap_or(&input)
        .trim_end_matches('.');
    let domain = match url::Host::parse(domain).map_err(|_| "Некорректное доменное имя")?
    {
        url::Host::Domain(domain) => domain,
        _ => return Err("Некорректный адрес исключения".into()),
    };
    if domain.len() > 253
        || (!domain.contains('.')
            && domain != "localhost"
            && domain != "ru"
            && domain != "xn--p1ai")
        || domain.parse::<std::net::Ipv4Addr>().is_ok()
        || domain.chars().all(|c| c.is_ascii_digit() || c == '.')
        || domain.split('.').any(|s| {
            s.is_empty()
                || s.len() > 63
                || s.starts_with('-')
                || s.ends_with('-')
                || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    {
        return Err("Введите домен, IP или CIDR: example.com / 192.168.0.0/16".into());
    }
    Ok(format!("DOMAIN-SUFFIX,{domain},DIRECT"))
}

/// Canonicalize saved route rules, including migration of old no-resolve IP
/// rules. Reject raw rule syntax and unknown targets before it reaches Mihomo.
pub fn normalize_direct_rules(rules: &[String]) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for rule in rules {
        let parts: Vec<_> = rule.split(',').collect();
        let target = parts.get(2).copied().unwrap_or_default();
        if !(parts.len() == 3
            || (parts.len() == 4 && parts[3] == "no-resolve" && parts[0].starts_with("IP-CIDR")))
            || !(target == "DIRECT"
                || target == "MULTILINK"
                || (target.len() <= 8192 && server_route_name(target).is_some())
                || target.strip_prefix("COUNTRY-").is_some_and(|code| {
                    code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_uppercase())
                })
                || (target.len() <= 128
                    && !target.is_empty()
                    && !target.starts_with("COUNTRY-")
                    && !target.starts_with("SERVER:")
                    && target.chars().all(|c| !c.is_control() && c != ',')))
            || ![
                "DOMAIN-SUFFIX",
                "IP-CIDR",
                "IP-CIDR6",
                "RULE-SET",
                "PROCESS-NAME",
                "PROCESS-PATH",
                "PROCESS-PATH-WILDCARD",
            ]
            .contains(&parts[0])
        {
            return Err("В списке исключений есть некорректное правило".into());
        }
        if parts[0] == "RULE-SET" {
            if parts[1] != "RU-SITES" || parts.len() != 3 {
                return Err("Неизвестный набор правил".into());
            }
            let canonical = format!("RULE-SET,RU-SITES,{target}");
            if seen.insert(rule_key(&canonical)) {
                result.push(canonical);
            }
            continue;
        }
        if parts[0] == "PROCESS-NAME" {
            let process = parts[1];
            let mac_name = cfg!(target_os = "macos");
            if process.is_empty()
                || process.len() > 128
                || (mac_name
                    && process
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '/' | '\\' | ',' | '*')))
                || (!mac_name
                    && (!process.to_ascii_lowercase().ends_with(".exe")
                        || process
                            .chars()
                            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ',' | '*' | '<' | '>' | ':' | '"' | '|' | '?'))))
            {
                return Err(if mac_name {
                    "Укажите имя процесса, например Safari, или полный путь к приложению"
                } else {
                    "Укажите имя приложения в формате program.exe"
                }
                .into());
            }
            let process = if mac_name && !process.to_ascii_lowercase().ends_with(".exe") {
                process.to_owned()
            } else {
                process.to_ascii_lowercase()
            };
            let canonical = format!("PROCESS-NAME,{process},{target}");
            if seen.insert(rule_key(&canonical)) {
                result.push(canonical);
            }
            continue;
        }
        if matches!(parts[0], "PROCESS-PATH" | "PROCESS-PATH-WILDCARD") {
            let path = parts[1];
            let wildcard = parts[0] == "PROCESS-PATH-WILDCARD";
            let checked = if wildcard {
                path.strip_suffix('*')
                    .ok_or("Некорректный путь приложения")?
            } else {
                path
            };
            if !std::path::Path::new(path).is_absolute()
                || path.len() > 4096
                || checked
                    .chars()
                    .any(|c| c.is_control() || matches!(c, ',' | '*' | '?' | '[' | ']'))
                || (wildcard && !checked.ends_with(".app/Contents/"))
                || std::path::Path::new(path)
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err("Укажите полный путь к исполняемому файлу приложения".into());
            }
            let canonical = format!("{},{path},{target}", parts[0]);
            if seen.insert(rule_key(&canonical)) {
                result.push(canonical);
            }
            continue;
        }
        let direct = direct_rule(parts[1])?;
        let canonical = format!("{},{target}", direct.rsplit_once(',').unwrap().0);
        if canonical.split(',').next() != Some(parts[0]) {
            return Err("Тип исключения не соответствует адресу".into());
        }
        if seen.insert(rule_key(&canonical)) {
            result.push(canonical);
        }
    }
    // An explicit site exception takes precedence over broad country presets,
    // regardless of the order in which the user added them.
    result.sort_by_key(|rule| {
        if rule.starts_with("PROCESS-") {
            0
        } else if rule.starts_with("RULE-SET,RU-SITES,") {
            2
        } else if rule.starts_with("DOMAIN-SUFFIX,ru,")
            || rule.starts_with("DOMAIN-SUFFIX,xn--p1ai,")
        {
            1
        } else {
            0
        }
    });
    Ok(result)
}

pub fn rules_from_input(input: &str, process: bool, target: &str) -> Result<Vec<String>, String> {
    let values: Vec<_> = input
        .split([',', ';', '\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if values.is_empty() {
        return Err("Введите домен или имя приложения".into());
    }
    if values.len() > 128 {
        return Err("За один раз можно добавить не более 128 адресов или приложений".into());
    }
    let mut rules = Vec::new();
    for value in values {
        let rule = if process
            && std::path::Path::new(value).is_absolute()
            && value.trim_end_matches('/').ends_with(".app")
        {
            format!(
                "PROCESS-PATH-WILDCARD,{}/Contents/*,{target}",
                value.trim_end_matches('/')
            )
        } else if process && value.ends_with(".app/Contents/*") {
            format!("PROCESS-PATH-WILDCARD,{value},{target}")
        } else if process && std::path::Path::new(value).is_absolute() {
            format!("PROCESS-PATH,{value},{target}")
        } else if process {
            format!("PROCESS-NAME,{value},{target}")
        } else {
            let base = direct_rule(value)?;
            format!("{},{target}", base.rsplit_once(',').unwrap().0)
        };
        rules.push(rule);
    }
    normalize_direct_rules(&rules)
}

/// Used to retire existing connections when a process rule changes.
pub fn process_rule_matches(rule: &str, process: &str, path: &str) -> bool {
    let mut parts = rule.split(',');
    let kind = parts.next().unwrap_or_default();
    let value = parts.next().unwrap_or_default();
    match kind {
        "PROCESS-NAME" => value.eq_ignore_ascii_case(process),
        "PROCESS-PATH" => value.eq_ignore_ascii_case(path),
        "PROCESS-PATH-WILDCARD" => value
            .strip_suffix('*')
            .is_some_and(|prefix| path.to_lowercase().starts_with(&prefix.to_lowercase())),
        _ => false,
    }
}

/// Country assignment uses labels supplied by the subscription. A node with
/// no unambiguous label stays out of country-specific groups.
pub const COUNTRIES: &[(&str, &str, &[&str])] = &[
    (
        "DE",
        "Германия",
        &["🇩🇪", "germany", "deutschland", "german", "германия", "de"],
    ),
    (
        "NL",
        "Нидерланды",
        &["🇳🇱", "netherlands", "holland", "нидерланды", "nl"],
    ),
    ("FI", "Финляндия", &["🇫🇮", "finland", "финляндия", "fi"]),
    ("SE", "Швеция", &["🇸🇪", "sweden", "швеция", "se"]),
    (
        "US",
        "США",
        &["🇺🇸", "usa", "united states", "america", "сша", "us"],
    ),
    (
        "GB",
        "Великобритания",
        &[
            "🇬🇧",
            "united kingdom",
            "uk",
            "britain",
            "великобритания",
            "gb",
        ],
    ),
    ("FR", "Франция", &["🇫🇷", "france", "франция", "fr"]),
    ("PL", "Польша", &["🇵🇱", "poland", "польша", "pl"]),
    ("TR", "Турция", &["🇹🇷", "turkey", "türkiye", "турция", "tr"]),
    (
        "RU",
        "Россия",
        &["🇷🇺", "russia", "россия", "ru", "rus", "москва", "moscow"],
    ),
    ("SG", "Сингапур", &["🇸🇬", "singapore", "сингапур", "sg"]),
    ("JP", "Япония", &["🇯🇵", "japan", "япония", "jp"]),
    ("CA", "Канада", &["🇨🇦", "canada", "канада", "ca"]),
    ("CH", "Швейцария", &["🇨🇭", "switzerland", "швейцария", "ch"]),
    ("UA", "Украина", &["🇺🇦", "ukraine", "украина", "ua"]),
    ("AE", "ОАЭ", &["🇦🇪", "uae", "emirates", "оаэ", "ae"]),
    ("AR", "Аргентина", &["🇦🇷", "argentina", "аргентина", "ar"]),
    ("AT", "Австрия", &["🇦🇹", "austria", "австрия", "at"]),
    ("AU", "Австралия", &["🇦🇺", "australia", "австралия", "au"]),
    ("BR", "Бразилия", &["🇧🇷", "brazil", "бразилия", "br"]),
    ("CY", "Кипр", &["🇨🇾", "cyprus", "кипр", "cy"]),
    ("DK", "Дания", &["🇩🇰", "denmark", "дания", "dk"]),
    ("EE", "Эстония", &["🇪🇪", "estonia", "эстония", "ee"]),
    ("EG", "Египет", &["🇪🇬", "egypt", "египет", "eg"]),
    ("ES", "Испания", &["🇪🇸", "spain", "испания", "es"]),
    ("GE", "Грузия", &["🇬🇪", "georgia", "грузия", "ge"]),
    (
        "HK",
        "Гонконг",
        &["🇭🇰", "hong kong", "hongkong", "гонконг", "hk"],
    ),
    ("IE", "Ирландия", &["🇮🇪", "ireland", "ирландия", "ie"]),
    ("IL", "Израиль", &["🇮🇱", "israel", "израиль", "il"]),
    ("IN", "Индия", &["🇮🇳", "india", "индия", "in"]),
    ("IT", "Италия", &["🇮🇹", "italy", "италия", "it"]),
    (
        "KR",
        "Южная Корея",
        &["🇰🇷", "south korea", "korea", "корея", "kr"],
    ),
    ("KZ", "Казахстан", &["🇰🇿", "kazakhstan", "казахстан", "kz"]),
    ("LV", "Латвия", &["🇱🇻", "latvia", "латвия", "lv"]),
    ("MD", "Молдова", &["🇲🇩", "moldova", "молдова", "md"]),
    ("MY", "Малайзия", &["🇲🇾", "malaysia", "малайзия", "my"]),
    ("NG", "Нигерия", &["🇳🇬", "nigeria", "нигерия", "ng"]),
    ("NO", "Норвегия", &["🇳🇴", "norway", "норвегия", "no"]),
    ("PH", "Филиппины", &["🇵🇭", "philippines", "филиппины", "ph"]),
    ("PT", "Португалия", &["🇵🇹", "portugal", "португалия", "pt"]),
    ("TH", "Таиланд", &["🇹🇭", "thailand", "таиланд", "th"]),
    ("TW", "Тайвань", &["🇹🇼", "taiwan", "тайвань", "tw"]),
    ("VN", "Вьетнам", &["🇻🇳", "vietnam", "вьетнам", "vn"]),
];
pub fn node_country(name: &str) -> Option<String> {
    let original_words: Vec<_> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let name = name.to_lowercase();
    let regional: Vec<_> = name
        .chars()
        .filter(|c| ('\u{1f1e6}'..='\u{1f1ff}').contains(c))
        .collect();
    if !regional.is_empty() {
        if regional.len() != 2 {
            return None;
        }
        return Some(
            regional
                .into_iter()
                .map(|c| char::from_u32('A' as u32 + (c as u32 - 0x1f1e6)).unwrap())
                .collect(),
        );
    }
    let words: Vec<_> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let mut matches = COUNTRIES.iter().filter(|(_, _, aliases)| {
        aliases.iter().any(|alias| {
            if alias.chars().count() == 2 && alias.is_ascii() {
                original_words.iter().any(|word| {
                    word.len() == 2
                        && word.chars().all(|c| c.is_ascii_uppercase())
                        && word.eq_ignore_ascii_case(alias)
                }) || (words.first() == Some(alias)
                    && (words.len() == 1
                        || words
                            .get(1)
                            .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()))))
            } else if alias.contains(' ') || alias.chars().any(|c| !c.is_alphanumeric()) {
                name.contains(alias)
            } else {
                words.contains(alias)
            }
        })
    });
    let first = matches.next()?.0;
    if matches.next().is_some() {
        None
    } else {
        Some(first.to_owned())
    }
}
pub fn available_countries(nodes: &[Node]) -> Vec<(String, String)> {
    let mut available: std::collections::BTreeSet<_> = nodes
        .iter()
        .filter(|node| node.enabled)
        .filter_map(|node| node_country(&node.name))
        .collect();
    let mut result = Vec::new();
    for (code, name, _) in COUNTRIES {
        if available.remove(*code) {
            result.push(((*code).to_owned(), (*name).to_owned()));
        }
    }
    result.extend(available.into_iter().map(|code| {
        let label = format!("Регион {code}");
        (code, label)
    }));
    result
}

pub fn is_russian_node(node: &Node) -> bool {
    if node_country(&node.name).as_deref() == Some("RU") {
        return true;
    }
    if node.name.contains("🇷🇺") {
        return true;
    }
    let lower = node.name.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    const RU_KEYWORDS: &[&str] = &[
        "russia",
        "россия",
        "российск",
        "moscow",
        "москва",
        "msk",
        "мск",
        "spb",
        "спб",
        "питер",
        "петербург",
        "petersburg",
        "novosibirsk",
        "новосибирск",
        "ekaterinburg",
        "екатеринбург",
        "kazan",
        "казань",
    ];
    if words
        .iter()
        .any(|w| RU_KEYWORDS.iter().any(|k| w.contains(k)))
    {
        return true;
    }
    for word in &words {
        let cleaned = word.trim_matches('-');
        if cleaned == "ru" || cleaned == "rus" {
            return true;
        }
        if cleaned.starts_with("ru")
            && cleaned.len() >= 3
            && cleaned[2..].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
        if cleaned.ends_with("ru")
            && cleaned.len() >= 3
            && cleaned[..cleaned.len() - 2]
                .chars()
                .all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    if let Some(server) = node
        .proxy
        .get(serde_yaml::Value::from("server"))
        .and_then(serde_yaml::Value::as_str)
    {
        let s = server.to_lowercase();
        if s.ends_with(".ru")
            || s.ends_with(".рф")
            || s.ends_with(".su")
            || s.ends_with(".xn--p1ai")
        {
            return true;
        }
    }
    false
}

/// Used to retire existing connections affected by a changed exception.
pub fn exception_matches(rule: &str, host: &str, ip: Option<std::net::IpAddr>) -> bool {
    let mut parts = rule.split(',');
    match (parts.next(), parts.next()) {
        (Some("DOMAIN-SUFFIX"), Some(domain)) => {
            let host = host.trim_end_matches('.').to_lowercase();
            host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        }
        (Some("IP-CIDR" | "IP-CIDR6"), Some(cidr)) => cidr
            .parse::<ipnet::IpNet>()
            .ok()
            .zip(ip)
            .is_some_and(|(net, ip)| net.contains(&ip)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_rules_validate_atomically_and_deduplicate() {
        assert_eq!(
            rules_from_input("one.exe, helper.exe; one.exe", true, "MULTILINK")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            rules_from_input("Example.com, https://other.example/path", false, "DIRECT").unwrap(),
            [
                "DOMAIN-SUFFIX,example.com,DIRECT",
                "DOMAIN-SUFFIX,other.example,DIRECT"
            ]
        );
        assert!(rules_from_input("good.exe, ../bad.exe", true, "DIRECT").is_err());
        assert!(rules_from_input("example.com, MATCH,DIRECT", false, "DIRECT").is_err());
    }
    #[test]
    fn routing_config_exports_only_routes_and_notes_and_merges_by_rule_key() {
        let mut settings = Settings::default();
        settings.rules = vec![
            "DOMAIN-SUFFIX,one.example,DIRECT".into(),
            "DOMAIN-SUFFIX,two.example,DIRECT".into(),
        ];
        settings
            .rule_notes
            .insert("DOMAIN-SUFFIX,one.example".into(), "old note".into());
        let exported = RoutingConfig::from_settings(&settings).encode().unwrap();
        let json: serde_json::Value = serde_json::from_str(&exported).unwrap();
        assert!(json.get("nodes").is_none() && json.get("tun").is_none());
        let mut imported = RoutingConfig::decode(&exported).unwrap();
        imported.rules = vec!["DOMAIN-SUFFIX,one.example,MULTILINK".into()];
        imported
            .notes
            .insert("DOMAIN-SUFFIX,one.example".into(), "new note".into());
        let merged = imported.merged(&settings);
        assert_eq!(merged.rules.len(), 2);
        assert!(merged
            .rules
            .contains(&"DOMAIN-SUFFIX,one.example,MULTILINK".into()));
        assert_eq!(merged.notes["DOMAIN-SUFFIX,one.example"], "new note");
        assert!(
            RoutingConfig::decode(&exported.replace("\"version\": 1", "\"version\": 99")).is_err()
        );
    }
    #[test]
    fn quality_uses_distinct_samples_and_penalizes_failures_and_jitter() {
        let mut fast = NodeQuality::default();
        let mut slow = NodeQuality::default();
        for i in 0..10 {
            fast.observe(&i.to_string(), Some(50));
            slow.observe(&i.to_string(), Some(1200));
        }
        assert!(fast.score() > slow.score());
        let samples = fast.samples;
        fast.observe("9", Some(1));
        assert_eq!(fast.samples, samples);
        let score = fast.score().unwrap();
        fast.observe("failure1", None);
        fast.observe("failure2", None);
        assert!(fast.score().unwrap() + 20 < score);
        for i in 0..10 {
            fast.observe(&format!("recover{i}"), Some(50));
        }
        assert!(fast.score().unwrap() > 80);
    }
    #[test]
    fn retired_dns_settings_are_ignored_without_resetting_connection_preferences() {
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old["tun"] = serde_json::json!(true);
        old["dns_ai"] = serde_json::json!(true);
        old["dns_provider"] = serde_json::json!("comss");
        let settings: Settings = serde_json::from_value(old).unwrap();
        assert!(settings.tun);
        let saved = serde_json::to_value(settings).unwrap();
        assert!(saved.get("dns_ai").is_none());
        assert!(saved.get("dns_provider").is_none());
    }
    const URI: &str = "vless://00000000-0000-4000-8000-000000000001@server.example:443?security=reality&sni=example.com&pbk=public&sid=abcd&type=ws&path=%2Fvless#Europe%20One";
    #[test]
    fn automatic_system_proxy_defaults_and_migrates_old_placeholder() {
        assert_eq!(Settings::default().system_proxy, default_system_proxy());
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut()
            .unwrap()
            .remove("automatic_system_proxy");
        old["system_proxy"] = serde_json::json!(false);
        assert_eq!(
            serde_json::from_value::<Settings>(old)
                .unwrap()
                .system_proxy,
            default_system_proxy()
        );
        let mut settings = Settings::default();
        settings.system_proxy = false;
        let roundtrip: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(!roundtrip.system_proxy);
    }
    #[test]
    fn preserves_reality_transport_and_name() {
        let n = parse_subscription(URI).unwrap().remove(0);
        assert_eq!(n.name, "Europe One");
        assert_eq!(n.proxy["ws-opts"]["path"], "/vless");
        assert_eq!(n.proxy["reality-opts"]["short-id"], "abcd");
    }
    #[test]
    fn base64_and_duplicates() {
        let nodes = parse_subscription(&general_purpose::STANDARD.encode(URI)).unwrap();
        let mut all = nodes.clone();
        assert_eq!(merge_nodes(&mut all, nodes), 0);
    }
    #[test]
    fn accepts_spider_path_and_preserves_hybrid_reality_flag() {
        let text = URI.replace(
            "type=ws",
            "spx=%2Ffallback&support-x25519mlkem768=true&type=ws",
        );
        let node = parse_subscription(&text).unwrap().remove(0);
        assert_eq!(node.spider_x.as_deref(), Some("/fallback"));
        assert_eq!(node.proxy["reality-opts"]["support-x25519mlkem768"], true);
        assert!(!node.proxy.contains_key(Value::from("spx")));
        // Existing protected libraries predate the optional source metadata.
        let mut json = serde_json::to_value(&node).unwrap();
        json.as_object_mut().unwrap().remove("spider_x");
        assert!(serde_json::from_value::<Node>(json)
            .unwrap()
            .spider_x
            .is_none());
    }
    #[test]
    fn mixed_import_reports_rejected_nodes_without_losing_working_ones() {
        let report = import_subscription(&general_purpose::STANDARD.encode(format!(
            "{URI}\n{}\n{}",
            URI.replace("type=ws", "type=invalid_proto"),
            URI.replace("type=ws", "fm=unsupported&type=tcp")
        )))
        .unwrap();
        assert_eq!(report.nodes.len(), 2);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("без фрагментации FM"));
        assert!(report.skipped[0].contains("Узел 2"));
        assert!(!report.skipped.join(" ").contains("server.example"));
        assert!(import_subscription("vless://broken").is_err());
    }
    #[test]
    fn rejects_partial_or_lossy_import() {
        assert!(parse_subscription(&format!("{URI}\nvless://broken")).is_err());
        assert!(parse_subscription(&URI.replace("type=ws", "type=invalid_proto")).is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_rules_support_names_and_bundle_executable_paths() {
        let path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let rules = rules_from_input(
            &format!("Safari; Google Chrome Helper; {path}"),
            true,
            "MULTILINK",
        )
        .unwrap();
        assert!(rules.contains(&"PROCESS-NAME,Safari,MULTILINK".into()));
        assert!(rules.contains(&"PROCESS-NAME,Google Chrome Helper,MULTILINK".into()));
        assert!(rules.contains(&format!("PROCESS-PATH,{path},MULTILINK")));
        assert!(rules_from_input("../Safari", true, "DIRECT").is_err());
        assert!(rules_from_input("/Applications/../bin/helper", true, "DIRECT").is_err());
        let mut settings = Settings::default();
        settings.rules = rules;
        let restored =
            RoutingConfig::decode(&RoutingConfig::from_settings(&settings).encode().unwrap())
                .unwrap();
        assert_eq!(restored.rules, settings.rules);
        let rule = rules_from_input("/Applications/Google Chrome.app", true, "DIRECT")
            .unwrap()
            .remove(0);
        assert!(rule.starts_with("PROCESS-PATH-WILDCARD,"));
        assert!(process_rule_matches(&rule, "Google Chrome Helper", "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper"));
        assert!(!process_rule_matches(
            &rule,
            "Other",
            "/Applications/Other.app/Contents/MacOS/Other"
        ));
        assert_eq!(normalize_direct_rules(&[rule.clone()]).unwrap(), vec![rule]);
    }
    #[test]
    fn windows_process_rules_support_names_with_spaces_and_symbols() {
        let rules = rules_from_input(
            "Telegram Desktop.exe; Dead by Daylight.exe; My Game (x64).exe",
            true,
            "MULTILINK",
        )
        .unwrap();
        assert!(rules.contains(&"PROCESS-NAME,telegram desktop.exe,MULTILINK".into()));
        assert!(rules.contains(&"PROCESS-NAME,dead by daylight.exe,MULTILINK".into()));
        assert!(rules.contains(&"PROCESS-NAME,my game (x64).exe,MULTILINK".into()));

        assert!(process_rule_matches(
            "PROCESS-NAME,telegram desktop.exe,MULTILINK",
            "Telegram Desktop.exe",
            "C:\\Program Files\\Telegram Desktop\\Telegram Desktop.exe"
        ));
    }
    #[test]
    fn fm_compatibility_import_preserves_connection_parameters() {
        let normal = parse_subscription(URI).unwrap().remove(0);
        let uri = URI.replace(
            "type=ws",
            "fm=%7B%22tcp%22%3A%5B%7B%22fragment%22%3A%22tlshello%22%7D%5D%7D&type=ws",
        );
        // Strict callers still reject lossy conversion. Interactive import
        // deliberately accepts FM omission and returns an explicit warning.
        assert!(parse_subscription(&uri).is_err());
        let mut payloads = vec![uri.clone()];
        for engine in [
            &general_purpose::STANDARD,
            &general_purpose::STANDARD_NO_PAD,
            &general_purpose::URL_SAFE,
            &general_purpose::URL_SAFE_NO_PAD,
        ] {
            payloads.push(engine.encode(&uri));
        }
        for payload in payloads {
            let report = import_subscription(&payload).unwrap();
            assert_eq!(report.nodes.len(), 1);
            assert!(report.skipped.is_empty());
            assert_eq!(report.warnings.len(), 1);
            let node = &report.nodes[0];
            assert_eq!(node.proxy, normal.proxy);
            assert_eq!(node.name, normal.name);
            assert_eq!(node.spider_x, normal.spider_x);
            assert!(!node.to_vless_uri().unwrap().contains("fm="));
            assert!(!report.warnings[0].contains("server.example"));
            assert!(!report.warnings[0].contains("tlshello"));
        }
        assert!(import_subscription(&uri.replace("type=ws", "type=invalid_proto")).is_err());
        assert!(import_subscription(&uri.replace("type=ws", "other=required&type=ws")).is_err());
    }
    #[test]
    fn direct_rules_are_validated() {
        assert_eq!(
            direct_rule("*.example.com").unwrap(),
            "DOMAIN-SUFFIX,example.com,DIRECT"
        );
        assert_eq!(direct_rule("::1").unwrap(), "IP-CIDR6,::1/128,DIRECT");
        assert!(direct_rule("example.com,DIRECT\nMATCH,DIRECT").is_err());
        assert!(direct_rule("999.123.2.1").is_err());
    }
    #[test]
    fn pasted_urls_and_idn_are_host_exceptions() {
        assert_eq!(
            direct_rule("https://Example.com:443/some/page?q=x#x").unwrap(),
            "DOMAIN-SUFFIX,example.com,DIRECT"
        );
        assert_eq!(
            direct_rule("example.com:8443/path").unwrap(),
            "DOMAIN-SUFFIX,example.com,DIRECT"
        );
        assert_eq!(
            direct_rule("https://[::1]:443/x").unwrap(),
            "IP-CIDR6,::1/128,DIRECT"
        );
        assert_eq!(
            direct_rule("пример.рф").unwrap(),
            "DOMAIN-SUFFIX,xn--e1afmkfd.xn--p1ai,DIRECT"
        );
        assert_eq!(
            direct_rule("EXAMPLE.COM.").unwrap(),
            "DOMAIN-SUFFIX,example.com,DIRECT"
        );
        assert!(direct_rule("ftp://example.com").is_err());
    }
    #[test]
    fn old_ip_exceptions_migrate_and_matching_obeys_domain_boundaries() {
        assert_eq!(
            normalize_direct_rules(&["IP-CIDR,192.168.0.0/16,DIRECT,no-resolve".into()]).unwrap(),
            vec!["IP-CIDR,192.168.0.0/16,DIRECT"]
        );
        let rule = direct_rule("example.com").unwrap();
        assert!(exception_matches(&rule, "www.EXAMPLE.com.", None));
        assert!(!exception_matches(&rule, "notexample.com", None));
        assert!(exception_matches(
            &direct_rule("10.0.0.0/8").unwrap(),
            "",
            Some("10.1.2.3".parse().unwrap())
        ));
        assert!(normalize_direct_rules(&["MATCH,DIRECT".into()]).is_err());
    }
    #[test]
    fn country_labels_are_unambiguous_and_explicit_rules_precede_ru_preset() {
        assert_eq!(node_country("🇩🇪 Germany · DE-01").as_deref(), Some("DE"));
        assert_eq!(node_country("Netherlands NL-02").as_deref(), Some("NL"));
        assert_eq!(node_country("🇩🇪 Germany 🇫🇮 Finland"), None);
        assert_eq!(
            node_country("🇺🇸 server in Singapore").as_deref(),
            Some("US")
        );
        assert_eq!(
            node_country("Singapore server in zone").as_deref(),
            Some("SG")
        );
        assert_eq!(node_country("🇨🇿 Прага").as_deref(), Some("CZ"));
        let mut nodes = parse_subscription(URI).unwrap();
        nodes[0].name = "🇨🇿 Прага".into();
        assert_eq!(
            available_countries(&nodes),
            vec![("CZ".to_owned(), "Регион CZ".to_owned())]
        );
        assert_eq!(node_country("generic server"), None);
        let rules = normalize_direct_rules(&[
            "RULE-SET,RU-SITES,DIRECT".into(),
            "DOMAIN-SUFFIX,ru,DIRECT".into(),
            "DOMAIN-SUFFIX,example.ru,COUNTRY-DE".into(),
        ])
        .unwrap();
        assert_eq!(
            rules,
            [
                "DOMAIN-SUFFIX,example.ru,COUNTRY-DE",
                "DOMAIN-SUFFIX,ru,DIRECT",
                "RULE-SET,RU-SITES,DIRECT"
            ]
        );
        assert!(normalize_direct_rules(&["DOMAIN-SUFFIX,example.com,COUNTRY-Z9".into()]).is_err());
    }
    #[test]
    fn test_russian_node_detection() {
        let mut n1 = parse_subscription(URI).unwrap().pop().unwrap();
        n1.name = "🇷🇺 Russia Fast 01".into();
        assert!(is_russian_node(&n1));

        let mut n2 = n1.clone();
        n2.name = "Moscow-02 HighSpeed".into();
        assert!(is_russian_node(&n2));

        let mut n3 = n1.clone();
        n3.name = "RU01-direct".into();
        assert!(is_russian_node(&n3));

        let mut n4 = n1.clone();
        n4.name = "Питер Сервер 1".into();
        assert!(is_russian_node(&n4));

        let mut n5 = n1.clone();
        n5.name = "🇩🇪 Germany Frankfurt".into();
        assert!(!is_russian_node(&n5));
    }
    #[test]
    fn test_expanded_protocol_parsing() {
        let hy2 = "hysteria2://pass123@hy2.example.com:27015?sni=hy2.example.com&obfs=salamander&obfs-password=secret#DE-Hy2";
        let node = parse_uri(hy2).unwrap();
        assert_eq!(node.name, "DE-Hy2");
        assert_eq!(node.proxy.get(Value::from("type")).unwrap().as_str().unwrap(), "hysteria2");
        assert_eq!(node.proxy.get(Value::from("password")).unwrap().as_str().unwrap(), "pass123");
        assert_eq!(node.proxy.get(Value::from("obfs")).unwrap().as_str().unwrap(), "salamander");

        let trojan = "trojan://password456@trojan.example.com:443?sni=trojan.example.com&type=ws&path=%2Fws#NL-Trojan";
        let node2 = parse_uri(trojan).unwrap();
        assert_eq!(node2.name, "NL-Trojan");
        assert_eq!(node2.proxy.get(Value::from("type")).unwrap().as_str().unwrap(), "trojan");
        assert_eq!(node2.proxy.get(Value::from("network")).unwrap().as_str().unwrap(), "ws");

        let ss = "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ=@ss.example.com:8388#US-SS";
        let node3 = parse_uri(ss).unwrap();
        assert_eq!(node3.name, "US-SS");
        assert_eq!(node3.proxy.get(Value::from("type")).unwrap().as_str().unwrap(), "ss");
        assert_eq!(node3.proxy.get(Value::from("cipher")).unwrap().as_str().unwrap(), "aes-256-gcm");

        let vmess_json = r#"{"v":"2","ps":"SE-VMess","add":"vmess.example.com","port":443,"id":"00000000-0000-4000-8000-000000000001","aid":0,"net":"ws","path":"/vmess","tls":"tls","sni":"vmess.example.com"}"#;
        let vmess = format!("vmess://{}", general_purpose::STANDARD.encode(vmess_json));
        let node4 = parse_uri(&vmess).unwrap();
        assert_eq!(node4.name, "SE-VMess");
        assert_eq!(node4.proxy.get(Value::from("type")).unwrap().as_str().unwrap(), "vmess");
        assert_eq!(node4.proxy.get(Value::from("network")).unwrap().as_str().unwrap(), "ws");

        let xhttp = "vless://00000000-0000-4000-8000-000000000001@xhttp.example.com:443?type=xhttp&mode=auto&path=/xh&security=reality&pbk=x25519pbk&sid=abcd#FI-XHTTP";
        let node5 = parse_uri(xhttp).unwrap();
        assert_eq!(node5.name, "FI-XHTTP");
        assert_eq!(node5.proxy.get(Value::from("network")).unwrap().as_str().unwrap(), "xhttp");
    }
}
