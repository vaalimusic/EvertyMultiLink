use super::*;
use std::net::{IpAddr, Ipv4Addr};

pub fn xray_vless(value: &serde_json::Value) -> Result<Vec<Node>, String> {
    let error = "В Xray JSON нет совместимого исходящего подключения VLESS";
    let outbounds = value["outbounds"].as_array().ok_or(error)?;
    let mut links = Vec::new();
    for outbound in outbounds.iter().filter(|v| v["protocol"] == "vless") {
        let stream = &outbound["streamSettings"];
        let network = match stream["network"].as_str().unwrap_or("tcp") {
            "tcp" | "raw" => "tcp",
            "ws" => "ws",
            "grpc" => "grpc",
            _ => return Err("Транспорт VLESS из Xray JSON не поддерживается".into()),
        };
        if [
            &stream["tcpSettings"]["header"],
            &stream["rawSettings"]["header"],
        ]
        .iter()
        .any(|header| header["type"].as_str().is_some_and(|kind| kind != "none"))
        {
            return Err("Маскировка TCP из Xray JSON не поддерживается".into());
        }
        let security = stream["security"].as_str().unwrap_or("none");
        let tls = &stream[format!("{security}Settings")];
        if tls["allowInsecure"].as_bool() == Some(true) {
            return Err("Импорт VLESS требует проверки TLS-сертификата".into());
        }
        for peer in outbound["settings"]["vnext"].as_array().ok_or(error)? {
            let address = peer["address"].as_str().ok_or(error)?;
            let port = peer["port"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or(error)?;
            for user in peer["users"].as_array().ok_or(error)? {
                let id = user["id"].as_str().ok_or(error)?;
                let mut url = Url::parse("vless://import.invalid").map_err(|_| error)?;
                url.set_host(Some(address)).map_err(|_| error)?;
                url.set_port(Some(port)).map_err(|_| error)?;
                url.set_username(id).map_err(|_| error)?;
                url.set_fragment(outbound["tag"].as_str().filter(|tag| !tag.is_empty()));
                {
                    let mut query = url.query_pairs_mut();
                    query
                        .append_pair("type", network)
                        .append_pair("security", security);
                    for (name, field) in [
                        ("sni", "serverName"),
                        ("fp", "fingerprint"),
                        ("sid", "shortId"),
                        ("spx", "spiderX"),
                        ("pqv", "mldsa65Verify"),
                    ] {
                        if let Some(value) = tls[field].as_str().filter(|v| !v.is_empty()) {
                            query.append_pair(name, value);
                        }
                    }
                    if let Some(value) = tls["publicKey"]
                        .as_str()
                        .or_else(|| tls["password"].as_str())
                    {
                        query.append_pair("pbk", value);
                    }
                    for field in ["flow", "encryption"] {
                        if let Some(value) = user[field].as_str().filter(|v| !v.is_empty()) {
                            query.append_pair(field, value);
                        }
                    }
                    if network == "ws" {
                        let ws = &stream["wsSettings"];
                        if let Some(path) = ws["path"].as_str() {
                            query.append_pair("path", path);
                        }
                        if let Some(host) = ws["host"]
                            .as_str()
                            .or_else(|| ws["headers"]["Host"].as_str())
                        {
                            query.append_pair("host", host);
                        }
                    } else if network == "grpc" {
                        if stream["grpcSettings"]["multiMode"] == true {
                            return Err("Режим gRPC multiMode не поддерживается".into());
                        }
                        if let Some(service) = stream["grpcSettings"]["serviceName"].as_str() {
                            query.append_pair("serviceName", service);
                        }
                    }
                }
                links.push(url.to_string());
                if links.len() > 2000 {
                    return Err("Лимит — 2000 серверов".into());
                }
            }
        }
    }
    if links.is_empty() {
        return Err(error.into());
    }
    parse_lines(&links.join("\n"))
}

pub const LAN_PORT: u16 = 18992;
pub const LAN_CIPHER: &str = "chacha20-ietf-poly1305";

fn node(name: &str, proxy: serde_json::Value) -> Result<Vec<Node>, String> {
    let proxy = serde_yaml::to_value(proxy).map_err(|_| "Некорректная конфигурация")?;
    let proxy = proxy
        .as_mapping()
        .cloned()
        .ok_or("Некорректная конфигурация")?;
    Ok(vec![Node {
        name: name.into(),
        enabled: true,
        proxy,
        spider_x: None,
        delay: None,
        quality: NodeQuality::default(),
        subscription_ids: Vec::new(),
        manually_added: true,
    }])
}

pub fn lan_node(input: &str) -> Result<Vec<Node>, String> {
    let error = "Нужна ссылка LAN-хоста Everty с частным IPv4 и ключом";
    let url = Url::parse(input).map_err(|_| error)?;
    let ip: Ipv4Addr = url.host_str().ok_or(error)?.parse().map_err(|_| error)?;
    let key = url.username();
    if !ip.is_private()
        || url.port() != Some(LAN_PORT)
        || key.len() != 32
        || !key.bytes().all(|b| b.is_ascii_hexdigit())
        || url.password().is_some()
        || !url.path().is_empty()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(error.into());
    }
    node(
        &format!("LAN · {ip}"),
        serde_json::json!({
            "type":"ss", "server":ip, "port":LAN_PORT, "cipher":LAN_CIPHER,
            "password":key, "udp":true
        }),
    )
}

pub fn is_lan_node(node: &Node) -> bool {
    node.proxy.get("type").and_then(Value::as_str) == Some("ss")
}

pub fn amnezia_conf(input: &str) -> Result<Vec<Node>, String> {
    let mut sections =
        std::collections::BTreeMap::<_, std::collections::BTreeMap<String, &str>>::new();
    let mut section = "";
    let error = "Некорректный .conf WireGuard/AmneziaWG. Нужен один [Interface] и один [Peer]";
    for line in input.lines() {
        let line = line.split(['#', ';']).next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        match line {
            "[Interface]" | "[Peer]" => {
                if sections.insert(line, Default::default()).is_some() {
                    return Err(error.into());
                }
                section = line;
            }
            _ => {
                let (key, value) = line.split_once('=').ok_or(error)?;
                if sections
                    .get_mut(section)
                    .ok_or(error)?
                    .insert(key.trim().to_ascii_lowercase(), value.trim())
                    .is_some()
                {
                    return Err(error.into());
                }
            }
        }
    }
    let interface = sections.get("[Interface]").ok_or(error)?;
    let peer = sections.get("[Peer]").ok_or(error)?;
    let key = |value: Option<&&str>| -> Result<String, String> {
        let value = *value.ok_or(error)?;
        if general_purpose::STANDARD
            .decode(value)
            .is_ok_and(|decoded| decoded.len() == 32)
        {
            Ok(value.into())
        } else {
            Err("В .conf указан некорректный ключ WireGuard".into())
        }
    };
    let (server, port) = peer
        .get("endpoint")
        .ok_or(error)?
        .rsplit_once(':')
        .ok_or(error)?;
    let server = server.trim_start_matches('[').trim_end_matches(']');
    let port = port.parse::<u16>().ok().filter(|p| *p != 0).ok_or(error)?;
    if url::Host::parse(server).is_err() && server.parse::<IpAddr>().is_err() {
        return Err(error.into());
    }
    let allowed: Vec<_> = peer
        .get("allowedips")
        .ok_or(error)?
        .split(',')
        .map(str::trim)
        .collect();
    if allowed.iter().any(|ip| ip.parse::<ipnet::IpNet>().is_err()) {
        return Err(error.into());
    }
    let mut proxy = serde_json::json!({
        "type":"wireguard", "server":server, "port":port, "udp":true,
        "private-key":key(interface.get("privatekey"))?, "public-key":key(peer.get("publickey"))?,
        "allowed-ips":allowed
    });
    for addr in interface.get("address").ok_or(error)?.split(',') {
        let net = addr.trim().parse::<ipnet::IpNet>().map_err(|_| error)?;
        let field = if net.addr().is_ipv4() { "ip" } else { "ipv6" };
        if !proxy[field].is_null() {
            return Err(error.into());
        }
        proxy[field] = serde_json::json!(net.addr());
    }
    if let Some(psk) = peer.get("presharedkey") {
        proxy["pre-shared-key"] = serde_json::json!(key(Some(psk))?);
    }
    if let Some(mtu) = interface.get("mtu") {
        proxy["mtu"] = serde_json::json!(mtu.parse::<u16>().map_err(|_| error)?);
    }
    if let Some(dns) = interface.get("dns") {
        let servers: Vec<IpAddr> = dns
            .split(',')
            .map(|ip| ip.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| error)?;
        proxy["dns"] = serde_json::json!(servers);
        proxy["remote-dns-resolve"] = serde_json::json!(true);
    }
    if let Some(value) = peer.get("persistentkeepalive") {
        if !value.eq_ignore_ascii_case("off") {
            let (low, high) = value.split_once('-').unwrap_or((value, value));
            let low = low.trim().parse::<u16>().map_err(|_| error)?;
            let high = high.trim().parse::<u16>().map_err(|_| error)?;
            if low > high {
                return Err(error.into());
            }
            proxy["persistent-keepalive"] = serde_json::json!(low + (high - low) / 2);
        }
    }
    let mut options = serde_json::Map::new();
    for (name, value) in interface {
        let number = matches!(
            name.as_str(),
            "jc" | "jmin" | "jmax" | "s1" | "s2" | "s3" | "s4" | "itime"
        );
        let field = match name.as_str() {
            "headerprotectionkey" => "header-protection-key",
            "contentpaddingaddition" => "content-padding-addition",
            "rekeyaftertime" => "rekey-after-time",
            "rekeytimeout" => "rekey-timeout",
            "rejectaftertime" => "reject-after-time",
            "keepalivetimeout" => "keepalive-timeout",
            "maxhandshakeattempts" => "max-handshake-attempts",
            "randomtrailers" => "random-trailers",
            "disablecookies" => "disable-cookies",
            "h1" | "h2" | "h3" | "h4" | "i1" | "i2" | "i3" | "i4" | "i5" | "j1" | "j2" | "j3" => {
                name
            }
            _ if number => name,
            "address" | "dns" | "privatekey" | "mtu" | "listenport" => continue,
            _ => return Err("В .conf есть неподдерживаемые параметры интерфейса".into()),
        };
        let value = if number {
            serde_json::json!(value.parse::<u32>().map_err(|_| error)?)
        } else if matches!(field, "random-trailers" | "disable-cookies") {
            serde_json::json!(match *value {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => return Err(error.into()),
            })
        } else {
            serde_json::json!(value)
        };
        options.insert(field.into(), value);
    }
    if peer.keys().any(|key| {
        !matches!(
            key.as_str(),
            "endpoint" | "publickey" | "presharedkey" | "allowedips" | "persistentkeepalive"
        )
    }) {
        return Err("В .conf есть неподдерживаемые параметры пира".into());
    }
    let is_awg = !options.is_empty();
    if is_awg {
        if options.keys().any(|field| field.contains('-')) {
            options.insert("version".into(), serde_json::json!(3));
        }
        proxy["amnezia-wg-option"] = options.into();
    }
    node(
        &format!(
            "{} · {server}",
            if is_awg { "AmneziaWG" } else { "WireGuard" }
        ),
        proxy,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_pairing_restricts_address_and_credentials() {
        let key = "0123456789abcdef0123456789abcdef";
        let link = |ip| format!("everty-lan://{key}@{ip}:{LAN_PORT}");
        let nodes = parse_subscription(&link("192.168.1.9")).unwrap();
        assert!(is_lan_node(&nodes[0]));
        assert!(parse_subscription(&link("203.0.113.9")).is_err());
        assert!(parse_subscription(&link("127.0.0.1")).is_err());
        assert!(parse_subscription(&format!("{}?key=no", link("10.0.0.9"))).is_err());
    }

    #[test]
    fn xray_client_outbound_keeps_vless_transport_and_tls() {
        let node = parse_subscription(
            r#"{
            "outbounds":[{"protocol":"vless","tag":"Xray","settings":{"vnext":[
              {"address":"example.invalid","port":443,"users":[
                {"id":"00000000-0000-4000-8000-000000000001","encryption":"none"}]}]},
              "streamSettings":{"network":"ws","security":"tls",
              "tlsSettings":{"serverName":"example.invalid"},"wsSettings":{"path":"/vpn"}}}]
        }"#,
        )
        .unwrap()
        .remove(0);
        assert_eq!(node.proxy["type"], "vless");
        assert_eq!(node.proxy["ws-opts"]["path"], "/vpn");
        assert_eq!(node.proxy["servername"], "example.invalid");
    }

    #[test]
    fn awg_config_preserves_v3_fields_and_imports_one_peer() {
        let key = general_purpose::STANDARD.encode([5; 32]);
        let input = format!("[Interface]\nAddress=10.0.0.2/32\nPrivateKey={key}\nHeaderProtectionKey={key}\nRekeyAfterTime=100-120\n\n[Peer]\nPublicKey={key}\nAllowedIPs=0.0.0.0/0\nEndpoint=peer.invalid:443\nPersistentKeepalive=20-30\n");
        let nodes = parse_subscription(&input).unwrap();
        let proxy = &nodes[0].proxy;
        assert_eq!(proxy["amnezia-wg-option"]["version"], 3);
        assert_eq!(proxy["amnezia-wg-option"]["rekey-after-time"], "100-120");
        assert_eq!(proxy["persistent-keepalive"], 25);
        assert!(parse_subscription(&format!("{input}[Peer]\n")).is_err());
    }
}
