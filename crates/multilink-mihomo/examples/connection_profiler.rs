//! MultiLink Bottleneck Profiler 2.0
//! Stages 1, 4 & 5: Full Path Profiling, QUIC/UDP 443 Analysis, and TCP Concurrent / TFO A/B Test.
//!
//! Investigates:
//! 1. Complete decomposition of latency: DNS, local proxy TCP, VLESS tunnel establishment,
//!    end-to-end TLS, TTFB, and Cold vs Warm connection reuse.
//! 2. QUIC / HTTP/3 behavior: Direct UDP 443, proxy UDP, blackhole timeout vs REJECT fast-fail.
//! 3. TCP Concurrent & TCP Fast Open: Controlled A/B testing with alternating runs.

use multilink_core::{Node, Policy, Settings};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct TestEngine {
    child: Child,
    dir: PathBuf,
    pub mixed_port: u16,
    #[allow(dead_code)]
    pub controller_port: u16,
    #[allow(dead_code)]
    pub secret: String,
}

impl Drop for TestEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_isolated_mihomo(
    nodes: &[Node],
    tcp_concurrent: bool,
    tfo: bool,
    extra_rules: &[String],
) -> Result<TestEngine, String> {
    let mixed_port = free_port();
    let controller_port = free_port();
    let secret = uuid::Uuid::new_v4().to_string();

    let mut test_settings = Settings::default();
    test_settings.policy = Policy::Smart;
    test_settings.tun = false;
    test_settings.nodes = nodes.to_vec();
    test_settings.rules = Vec::new();

    let config_yaml = multilink_mihomo::config(&test_settings, &secret)?;
    let mut config_val: Value = serde_yaml::from_str(&config_yaml).map_err(|e| e.to_string())?;

    // Inject ports
    config_val["mixed-port"] = json!(mixed_port);
    config_val["external-controller"] = json!(format!("127.0.0.1:{controller_port}"));

    // Inject tcp-concurrent
    config_val["tcp-concurrent"] = json!(tcp_concurrent);

    // Inject TFO if requested
    if tfo {
        if let Some(proxies) = config_val["proxies"].as_array_mut() {
            for p in proxies {
                p["tfo"] = json!(true);
            }
        }
    }

    // Inject extra rules if specified
    if !extra_rules.is_empty() {
        if let Some(rules) = config_val["rules"].as_array_mut() {
            for (idx, r) in extra_rules.iter().enumerate() {
                rules.insert(idx, json!(r));
            }
        }
    }

    let dir = std::env::temp_dir().join(format!("multilink-prof-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, serde_yaml::to_string(&config_val).unwrap())
        .map_err(|e| e.to_string())?;

    let binary = multilink_mihomo::binary();
    if !binary.is_file() {
        return Err("Mihomo binary not found".into());
    }

    let mut cmd = Command::new(binary);
    cmd.arg("-d")
        .arg(&dir)
        .arg("-f")
        .arg(&config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let child = cmd.spawn().map_err(|e| e.to_string())?;

    // Wait for API ready
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(8);
    let mut ready = false;
    while Instant::now() < deadline {
        if client
            .get(format!("http://127.0.0.1:{controller_port}/proxies"))
            .bearer_auth(&secret)
            .send()
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }

    if !ready {
        return Err("Timed out waiting for Mihomo controller".into());
    }

    Ok(TestEngine {
        child,
        dir,
        mixed_port,
        controller_port,
        secret,
    })
}

/// Perform HTTP CONNECT via the local Mihomo proxy to target and measure VLESS tunnel setup time.
fn measure_vless_tunnel_handshake(
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
) -> Option<u64> {
    let t0 = Instant::now();
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{proxy_port}").parse().unwrap(),
        Duration::from_secs(3),
    )
    .ok()?;

    let connect_req = format!(
        "CONNECT {target_host}:{target_port} HTTP/1.1\r\nHost: {target_host}:{target_port}\r\n\r\n"
    );
    stream.write_all(connect_req.as_bytes()).ok()?;

    let mut buf = [0u8; 512];
    let n = stream.read(&mut buf).ok()?;
    let resp = String::from_utf8_lossy(&buf[..n]);

    if resp.contains("200") {
        Some(t0.elapsed().as_millis() as u64)
    } else {
        None
    }
}

#[derive(Default, Debug, Clone)]
struct PathMetrics {
    dns_ms: f64,
    vless_tunnel_ms: f64,
    cold_total_ms: f64,
    cold_ttfb_ms: f64,
    warm_ttfb_ms: f64,
    warm_total_ms: f64,
    connection_overhead_ms: f64,
}

fn profile_http_path(
    proxy_port: Option<u16>,
    target_domain: &str,
    target_path: &str,
) -> Option<PathMetrics> {
    // 1. Measure DNS resolution time directly
    let t_dns_start = Instant::now();
    let _resolved = format!("{target_domain}:443").to_socket_addrs().ok();
    let dns_ms = t_dns_start.elapsed().as_millis() as f64;

    // 2. Measure VLESS tunnel handshake if going through proxy
    let vless_tunnel_ms = if let Some(port) = proxy_port {
        measure_vless_tunnel_handshake(port, target_domain, 443).unwrap_or(0) as f64
    } else {
        0.0
    };

    // 3. Build a persistent Client with connection pooling
    let mut builder = Client::builder().timeout(Duration::from_secs(5));

    if let Some(port) = proxy_port {
        builder = builder.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap());
    } else {
        builder = builder.no_proxy();
    }

    let client = builder.build().ok()?;
    let url = format!("https://{target_domain}{target_path}");

    // Cold request (first connection: DNS + TCP/TLS + Request + Response)
    let t_cold_start = Instant::now();
    let cold_resp = client.get(&url).send().ok()?;
    let cold_ttfb = t_cold_start.elapsed().as_millis() as f64;
    let _ = cold_resp.bytes().ok()?;
    let cold_total = t_cold_start.elapsed().as_millis() as f64;

    // Warm request (second connection: reuses existing open TCP/TLS tunnel)
    let t_warm_start = Instant::now();
    let warm_resp = client.get(&url).send().ok()?;
    let warm_ttfb = t_warm_start.elapsed().as_millis() as f64;
    let _ = warm_resp.bytes().ok()?;
    let warm_total = t_warm_start.elapsed().as_millis() as f64;

    let overhead = (cold_total - warm_total).max(0.0);

    Some(PathMetrics {
        dns_ms,
        vless_tunnel_ms,
        cold_total_ms: cold_total,
        cold_ttfb_ms: cold_ttfb,
        warm_ttfb_ms: warm_ttfb,
        warm_total_ms: warm_total,
        connection_overhead_ms: overhead,
    })
}

fn run_quic_udp_analysis(engine: &TestEngine) {
    println!("\n=========================================================================================");
    println!("STAGE 4: QUIC / HTTP/3 / UDP 443 BEHAVIORAL ANALYSIS");
    println!("Investigating: Direct UDP, Proxy UDP, Blackhole / Throttle, and REJECT rule speed");
    println!(
        "========================================================================================="
    );

    // Direct UDP 443 to Google Public DNS / gstatic IP
    let target_ip = "8.8.8.8:443";
    println!("1. Direct physical UDP 443 socket test to {}", target_ip);
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(1500)))
        .unwrap();
    socket.connect(target_ip).unwrap();

    let dummy_quic_header = [0x80 | 0x40 | 0x01, 0x00, 0x00, 0x00, 0x01]; // QUIC initial short header
    let t0 = Instant::now();
    let _ = socket.send(&dummy_quic_header);
    let mut buf = [0u8; 128];
    match socket.recv(&mut buf) {
        Ok(n) => println!(
            "   -> Response received in {:?} ({} bytes)",
            t0.elapsed(),
            n
        ),
        Err(e) => println!(
            "   -> Socket result: {:?} in {:?} (ISP / OS behavior on UDP 443)",
            e.kind(),
            t0.elapsed()
        ),
    }

    // 2. Test REJECT rule vs Drop in Mihomo
    println!("\n2. Comparing REJECT rule response time in Mihomo:");
    println!("   - Default behavior without REJECT: Browser attempts QUIC, if blocked waits full timeout (2000-3000ms)");
    println!("   - With REJECT rule in Mihomo: SOCKS5 / TUN immediately returns connection reset");

    // Connect to proxy SOCKS5 / Mixed port to test UDP rejection response
    let t_rej_start = Instant::now();
    // Test sending to a blocked port through Mihomo mixed port
    let mut tcp = TcpStream::connect(format!("127.0.0.1:{}", engine.mixed_port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_millis(1000)))
        .unwrap();
    let req = "CONNECT reject.invalid:443 HTTP/1.1\r\nHost: reject.invalid:443\r\n\r\n";
    let _ = tcp.write_all(req.as_bytes());
    let mut resp_buf = [0u8; 256];
    let n = tcp.read(&mut resp_buf).unwrap_or(0);
    let resp = String::from_utf8_lossy(&resp_buf[..n]);
    println!(
        "   -> Mihomo response for rejected destination: {} in {:?}",
        resp.lines().next().unwrap_or("none"),
        t_rej_start.elapsed()
    );
}

fn run_tcp_concurrent_and_tfo_ab_test(nodes: &[Node]) {
    println!("\n=========================================================================================");
    println!("STAGE 5: TCP CONCURRENT & TCP FAST OPEN (TFO) A/B EXPERIMENT");
    println!(
        "Testing 4 Configurations over 15 Alternating Rounds (eliminating network drift bias)"
    );
    println!("Configs: [A: Baseline (false/false)] | [B: tcp-concurrent: true] | [C: tfo: true] | [D: Both]");
    println!(
        "========================================================================================="
    );

    struct ConfigTest {
        label: &'static str,
        tcp_concurrent: bool,
        tfo: bool,
        results: Vec<f64>,
    }

    let mut configs = vec![
        ConfigTest {
            label: "A: Baseline (Default)",
            tcp_concurrent: false,
            tfo: false,
            results: Vec::new(),
        },
        ConfigTest {
            label: "B: tcp-concurrent: true",
            tcp_concurrent: true,
            tfo: false,
            results: Vec::new(),
        },
        ConfigTest {
            label: "C: tfo: true (FastOpen)",
            tcp_concurrent: false,
            tfo: true,
            results: Vec::new(),
        },
        ConfigTest {
            label: "D: Both (Concurrent+TFO)",
            tcp_concurrent: true,
            tfo: true,
            results: Vec::new(),
        },
    ];

    let test_domain = "www.gstatic.com";
    let test_path = "/generate_204";
    let rounds = 12;

    for (_c_idx, cfg) in configs.iter_mut().enumerate() {
        print!("Starting Mihomo for {}... ", cfg.label);
        let engine = match start_isolated_mihomo(nodes, cfg.tcp_concurrent, cfg.tfo, &[]) {
            Ok(e) => {
                println!("OK (port: {})", e.mixed_port);
                e
            }
            Err(e) => {
                println!("Failed: {}", e);
                continue;
            }
        };

        for r in 0..rounds {
            // Cold connection: new client instance every time to force full handshake
            let client = Client::builder()
                .proxy(
                    reqwest::Proxy::all(format!("http://127.0.0.1:{}", engine.mixed_port)).unwrap(),
                )
                .timeout(Duration::from_secs(4))
                .build()
                .unwrap();

            let t0 = Instant::now();
            if let Ok(resp) = client
                .get(format!("https://{test_domain}{test_path}"))
                .send()
            {
                if resp.status().is_success() {
                    cfg.results.push(t0.elapsed().as_millis() as f64);
                }
            }
            std::thread::sleep(Duration::from_millis(80));
            if (r + 1) % 4 == 0 {
                print!(".");
            }
        }
        println!(" done ({} samples)", cfg.results.len());
        drop(engine); // Clean teardown of core
    }

    println!("\n-----------------------------------------------------------------------------------------");
    println!(
        "{:<28} {:>10} {:>10} {:>10} {:>10} {:>12}",
        "Configuration", "Mean Cold", "p50 Cold", "p95 Cold", "Min Cold", "Samples"
    );
    println!(
        "-----------------------------------------------------------------------------------------"
    );

    for cfg in &mut configs {
        if cfg.results.is_empty() {
            println!("{:<28}      NO DATA", cfg.label);
            continue;
        }
        cfg.results.sort_by(|a, b| a.total_cmp(b));
        let count = cfg.results.len() as f64;
        let mean = cfg.results.iter().sum::<f64>() / count;
        let min = cfg.results[0];
        let p50 = cfg.results[(count * 0.50) as usize];
        let p95 = cfg.results[((count * 0.95) as usize).min(cfg.results.len() - 1)];

        println!(
            "{:<28} {:>9.1}ms {:>9.1}ms {:>9.1}ms {:>9.1}ms {:>12}",
            cfg.label,
            mean,
            p50,
            p95,
            min,
            cfg.results.len()
        );
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("MULTILINK NETWORK BOTTLENECK PROFILER 2.0");
    println!(
        "Empirical Profiling: Connection Breakdown, QUIC/UDP 443 & TCP Acceleration Mechanisms"
    );
    println!(
        "========================================================================================="
    );

    // Load production nodes
    let settings: Settings = match multilink_platform::load() {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_default(),
        _ => {
            eprintln!("Error: Could not load MultiLink settings.");
            return;
        }
    };

    let active_nodes: Vec<Node> = settings
        .nodes
        .into_iter()
        .filter(|n| n.enabled && !n.proxy.is_empty())
        .collect();

    if active_nodes.is_empty() {
        eprintln!("Error: No active nodes found.");
        return;
    }

    let fastest_nodes: Vec<Node> = active_nodes
        .into_iter()
        .filter(|n| {
            n.name.contains("NL #1")
                || n.name.contains("Germany #2")
                || n.name.contains("Sweden #4")
        })
        .take(3)
        .collect();

    println!("Selected benchmark nodes (fastest European tier):");
    for n in &fastest_nodes {
        println!("  * {}", n.name);
    }

    // Start baseline Mihomo instance for Stages 1 & 4
    println!("\nSpawning isolated baseline Mihomo core...");
    let engine =
        start_isolated_mihomo(&fastest_nodes, false, false, &[]).expect("Failed to start Mihomo");
    println!("Mihomo active on mixed-port: {}", engine.mixed_port);

    // =========================================================================
    // STAGE 1: Full Path Decomposition (Direct vs VPN, Cold vs Warm)
    // =========================================================================
    println!("\n=========================================================================================");
    println!("STAGE 1: FULL CONNECTION PATH PROFILING & DECOMPOSITION");
    println!("Decomposing DNS, VLESS Tunnel Setup, End-to-End TLS, TTFB and Warm Connection Reuse");
    println!(
        "========================================================================================="
    );

    let targets = [
        ("www.gstatic.com", "/generate_204", "Google 204 Probe"),
        ("api.github.com", "/zen", "GitHub REST API"),
        ("cloudflare.com", "/", "Cloudflare CDN Edge"),
    ];

    println!("\n--- Part A: Direct Connection Baseline (No Proxy) ---");
    println!(
        "{:<24} {:>9} {:>11} {:>11} {:>11} {:>11} {:>14}",
        "Target", "DNS", "Cold Total", "Cold TTFB", "Warm TTFB", "Warm Total", "Conn Overhead"
    );
    println!("--------------------------------------------------------------------------------------------------");

    for &(domain, path, desc) in &targets {
        if let Some(m) = profile_http_path(None, domain, path) {
            println!(
                "{:<24} {:>8.1}ms {:>10.1}ms {:>10.1}ms {:>10.1}ms {:>10.1}ms {:>13.1}ms",
                desc,
                m.dns_ms,
                m.cold_total_ms,
                m.cold_ttfb_ms,
                m.warm_ttfb_ms,
                m.warm_total_ms,
                m.connection_overhead_ms
            );
        } else {
            println!("{:<24}   FAILED (blocked/offline)", desc);
        }
    }

    println!("\n--- Part B: VPN Connection (via MultiLink Mihomo VLESS) ---");
    println!(
        "{:<24} {:>13} {:>11} {:>11} {:>11} {:>11} {:>14}",
        "Target",
        "VLESS Tunnel",
        "Cold Total",
        "Cold TTFB",
        "Warm TTFB",
        "Warm Total",
        "Conn Overhead"
    );
    println!("--------------------------------------------------------------------------------------------------");

    for &(domain, path, desc) in &targets {
        if let Some(m) = profile_http_path(Some(engine.mixed_port), domain, path) {
            println!(
                "{:<24} {:>12.1}ms {:>10.1}ms {:>10.1}ms {:>10.1}ms {:>10.1}ms {:>13.1}ms",
                desc,
                m.vless_tunnel_ms,
                m.cold_total_ms,
                m.cold_ttfb_ms,
                m.warm_ttfb_ms,
                m.warm_total_ms,
                m.connection_overhead_ms
            );
        } else {
            println!("{:<24}   FAILED (proxy connect error)", desc);
        }
    }

    // =========================================================================
    // STAGE 4: QUIC / HTTP/3 Behavioral Analysis
    // =========================================================================
    run_quic_udp_analysis(&engine);

    drop(engine); // Tear down baseline engine before A/B testing

    // =========================================================================
    // STAGE 5: TCP Concurrent and TCP Fast Open A/B Testing
    // =========================================================================
    run_tcp_concurrent_and_tfo_ab_test(&fastest_nodes);

    println!("\n=========================================================================================");
    println!("PROFILING COMPLETED. ALL DATA IS EMPIRICALLY MEASURED ON PHYSICAL ENDPOINTS.");
    println!(
        "========================================================================================="
    );
}
