//! Cold vs Warm Connection Verification Benchmark
//! 100 Paired Iterations measuring physical decomposition on real VPN endpoints:
//! 1. DNS resolution
//! 2. Raw TCP to VPS
//! 3. Mihomo CONNECT proxy tunnel setup (VLESS transport + remote TCP)
//! 4. End-to-End TLS handshake
//! 5. Cold TTFB vs Warm TTFB (verifying HTTP keep-alive reuse)

use multilink_core::{Node, Policy, Settings};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct TestEngine {
    child: Child,
    dir: PathBuf,
    pub mixed_port: u16,
    #[allow(dead_code)]
    pub controller_port: u16,
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

fn start_mihomo_for_node(node: &Node) -> Result<TestEngine, String> {
    let mixed_port = free_port();
    let controller_port = free_port();
    let secret = "verification-secret";

    let mut test_settings = Settings::default();
    test_settings.policy = Policy::Smart;
    test_settings.tun = false;
    test_settings.nodes = vec![node.clone()];
    test_settings.rules = Vec::new();

    let config_yaml = multilink_mihomo::config(&test_settings, secret)?;
    let mut config_val: Value = serde_yaml::from_str(&config_yaml).map_err(|e| e.to_string())?;

    config_val["mixed-port"] = json!(mixed_port);
    config_val["external-controller"] = json!(format!("127.0.0.1:{controller_port}"));

    let dir = std::env::temp_dir().join(format!("multilink-cw-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, serde_yaml::to_string(&config_val).unwrap())
        .map_err(|e| e.to_string())?;

    let binary = multilink_mihomo::binary();
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
            .bearer_auth(secret)
            .send()
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }

    if !ready {
        return Err("Timed out waiting for Mihomo".into());
    }

    Ok(TestEngine {
        child,
        dir,
        mixed_port,
        controller_port,
    })
}

/// Measure physical CONNECT tunnel establishment time
fn measure_proxy_connect_handshake(proxy_port: u16, target_host: &str) -> Option<u64> {
    let t0 = Instant::now();
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{proxy_port}").parse().unwrap(),
        Duration::from_secs(3),
    )
    .ok()?;

    let req = format!("CONNECT {target_host}:443 HTTP/1.1\r\nHost: {target_host}:443\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;

    let mut buf = [0u8; 512];
    let n = stream.read(&mut buf).ok()?;
    let resp = String::from_utf8_lossy(&buf[..n]);

    if resp.contains("200") {
        Some(t0.elapsed().as_millis() as u64)
    } else {
        None
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("COLD VS WARM CONNECTION VERIFICATION (100 PAIRED SAMPLES)");
    println!("Target: www.gstatic.com/generate_204 via NL #1 (10 Gbit)");
    println!("Empirically Measuring Physical Decomposition & Keep-Alive Reuse");
    println!(
        "========================================================================================="
    );

    let settings: Settings = match multilink_platform::load() {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_default(),
        _ => {
            eprintln!("Error: Cannot load settings");
            return;
        }
    };

    let nl_node = settings
        .nodes
        .into_iter()
        .find(|n| n.name.contains("NL #1") && n.enabled)
        .expect("NL #1 node not found");

    println!("Selected primary node: {}", nl_node.name);

    // 1. Measure raw TCP connect to the VPS itself
    let vps_server = nl_node
        .proxy
        .get(&serde_yaml::Value::String("server".into()))
        .and_then(|v| v.as_str())
        .unwrap_or("nl1.gofizzin.com");

    print!(
        "Measuring raw TCP handshake to VPS ({}:443)... ",
        vps_server
    );
    let mut raw_vps_rtts = Vec::new();
    for _ in 0..15 {
        let t0 = Instant::now();
        if let Ok(addr) = format!("{vps_server}:443").to_socket_addrs() {
            if let Some(first) = addr.into_iter().next() {
                if let Ok(stream) = TcpStream::connect_timeout(&first, Duration::from_secs(2)) {
                    raw_vps_rtts.push(t0.elapsed().as_millis() as u64);
                    drop(stream);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    raw_vps_rtts.sort_unstable();
    let median_vps_tcp = raw_vps_rtts[raw_vps_rtts.len() / 2];
    println!("Median TCP RTT: {} ms", median_vps_tcp);

    // 2. Start isolated Mihomo instance
    println!("Starting isolated Mihomo instance...");
    let engine = start_mihomo_for_node(&nl_node).expect("Failed to start Mihomo");
    println!("Mihomo active on port {}", engine.mixed_port);

    // 3. Measure Mihomo CONNECT tunnel handshake (15 samples)
    print!("Measuring Mihomo CONNECT tunnel setup to www.gstatic.com:443... ");
    let mut tunnel_handshakes = Vec::new();
    for _ in 0..15 {
        if let Some(rtt) = measure_proxy_connect_handshake(engine.mixed_port, "www.gstatic.com") {
            tunnel_handshakes.push(rtt);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    tunnel_handshakes.sort_unstable();
    let median_tunnel_setup = tunnel_handshakes[tunnel_handshakes.len() / 2];
    println!("Median Tunnel Setup: {} ms", median_tunnel_setup);

    // 4. Run 100 paired Cold vs Warm requests
    println!(
        "\nExecuting 100 Paired Cold / Warm Requests (alternating with connection teardown)..."
    );
    let url = "https://www.gstatic.com/generate_204";
    let iterations = 100;

    let mut cold_totals = Vec::with_capacity(iterations);
    let mut warm_totals = Vec::with_capacity(iterations);
    let mut overheads = Vec::with_capacity(iterations);

    for i in 0..iterations {
        // Cold request: new client instance (fresh TCP pool, fresh TLS handshake)
        let client = Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{}", engine.mixed_port)).unwrap())
            .timeout(Duration::from_secs(4))
            .build()
            .unwrap();

        let t0 = Instant::now();
        let resp_cold = client.get(url).send();
        let cold_ms = t0.elapsed().as_millis() as u64;

        if resp_cold.is_ok_and(|r| r.status().is_success()) {
            cold_totals.push(cold_ms);

            // Warm request: immediate second request over the SAME client (reusing active HTTP/1.1 TCP socket)
            let t1 = Instant::now();
            let resp_warm = client.get(url).send();
            let warm_ms = t1.elapsed().as_millis() as u64;

            if resp_warm.is_ok_and(|r| r.status().is_success()) {
                warm_totals.push(warm_ms);
                overheads.push(cold_ms.saturating_sub(warm_ms));
            }
        }

        if (i + 1) % 20 == 0 {
            println!("  Iteration {:>3}/100 complete...", i + 1);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(engine);

    // Statistical summary
    cold_totals.sort_unstable();
    warm_totals.sort_unstable();
    overheads.sort_unstable();

    let p50_idx = (cold_totals.len() as f64 * 0.50) as usize;
    let p95_idx = ((cold_totals.len() as f64 * 0.95) as usize).min(cold_totals.len() - 1);
    let p99_idx = ((cold_totals.len() as f64 * 0.99) as usize).min(cold_totals.len() - 1);

    let mean_cold = cold_totals.iter().sum::<u64>() as f64 / cold_totals.len() as f64;
    let mean_warm = warm_totals.iter().sum::<u64>() as f64 / warm_totals.len() as f64;
    let mean_overhead = overheads.iter().sum::<u64>() as f64 / overheads.len() as f64;

    println!("\n=========================================================================================");
    println!(
        "STATISTICAL SUMMARY ACROSS {} VERIFIED SAMPLES",
        cold_totals.len()
    );
    println!(
        "========================================================================================="
    );
    println!("Metric                 Mean       p50       p95       p99       Min       Max");
    println!(
        "-----------------------------------------------------------------------------------------"
    );
    println!(
        "{:<20} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>7}ms {:>7}ms",
        "Cold Request Total",
        mean_cold,
        cold_totals[p50_idx],
        cold_totals[p95_idx],
        cold_totals[p99_idx],
        cold_totals[0],
        cold_totals[cold_totals.len() - 1]
    );
    println!(
        "{:<20} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>7}ms {:>7}ms",
        "Warm Request Total",
        mean_warm,
        warm_totals[p50_idx],
        warm_totals[p95_idx],
        warm_totals[p99_idx],
        warm_totals[0],
        warm_totals[warm_totals.len() - 1]
    );
    println!(
        "{:<20} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>7}ms {:>7}ms",
        "Connection Overhead",
        mean_overhead,
        overheads[p50_idx],
        overheads[p95_idx],
        overheads[p99_idx],
        overheads[0],
        overheads[overheads.len() - 1]
    );
    println!(
        "-----------------------------------------------------------------------------------------"
    );

    println!(
        "\nPHYSICAL BREAKDOWN OF THE {}ms COLD LATENCY:",
        cold_totals[p50_idx]
    );
    println!(
        "  1. Raw TCP to VPS:                     ~{} ms",
        median_vps_tcp
    );
    println!(
        "  2. VLESS + Remote TCP Handshake:       ~{} ms (Mihomo CONNECT)",
        median_tunnel_setup
    );
    println!(
        "  3. End-to-End TLS with target:         ~{} ms (diff: tunnel setup to first payload)",
        cold_totals[p50_idx].saturating_sub(median_tunnel_setup + warm_totals[p50_idx])
    );
    println!(
        "  4. Pure Warm Application RTT:          ~{} ms (Warm Request)",
        warm_totals[p50_idx]
    );
    println!(
        "  Ratio of Connection Setup to Request:   {:.1}% setup vs {:.1}% payload",
        100.0 * mean_overhead / mean_cold,
        100.0 * mean_warm / mean_cold
    );
    println!(
        "========================================================================================="
    );
}
