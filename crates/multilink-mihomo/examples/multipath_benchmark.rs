//! MultiPath Egress Benchmark
//! Comparing:
//! 1. Single Best Server (Smart Mode: NL #1)
//! 2. Load-Balance Round-Robin (NL #1 + Germany #2 + Sweden #4)
//! 3. Load-Balance Consistent-Hashing (NL #1 + Germany #2 + Sweden #4)
//!
//! Evaluates:
//! - Multi-stream parallel download completion time
//! - Multi-component page fetch latency (12 assets across multiple domains)
//! - Egress IP stability across sequential requests to the same domain (session stickiness)

use multilink_core::{Node, Policy, Settings};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::net::TcpListener;
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

fn start_mihomo_with_group(
    nodes: &[Node],
    group_type: &str,       // "load-balance" or "url-test"
    strategy: Option<&str>, // "consistent-hashing", "round-robin", or None
) -> Result<TestEngine, String> {
    let mixed_port = free_port();
    let controller_port = free_port();
    let secret = "multipath-secret";

    let mut test_settings = Settings::default();
    test_settings.policy = Policy::Smart;
    test_settings.tun = false;
    test_settings.nodes = nodes.to_vec();
    test_settings.rules = Vec::new();

    let config_yaml = multilink_mihomo::config(&test_settings, secret)?;
    let mut config_val: Value = serde_yaml::from_str(&config_yaml).map_err(|e| e.to_string())?;

    config_val["mixed-port"] = json!(mixed_port);
    config_val["external-controller"] = json!(format!("127.0.0.1:{controller_port}"));

    // Modify the MULTILINK proxy group to test the requested strategy
    if let Some(groups) = config_val["proxy-groups"].as_array_mut() {
        for g in groups {
            if g["name"] == "MULTILINK" {
                g["type"] = json!(group_type);
                if let Some(strat) = strategy {
                    g["strategy"] = json!(strat);
                    g.as_object_mut().unwrap().remove("tolerance");
                }
            }
        }
    }

    let dir = std::env::temp_dir().join(format!("multilink-mp-{}", uuid::Uuid::new_v4()));
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

#[derive(Default, Debug, Clone)]
struct MultiPathResults {
    strategy_name: &'static str,
    parallel_download_total_ms: f64,
    page_load_total_ms: f64,
    page_component_p95_ms: f64,
    ip_changes_on_same_domain: usize,
    successful_requests: usize,
    failed_requests: usize,
}

fn benchmark_engine(strategy_name: &'static str, engine: &TestEngine) -> MultiPathResults {
    let proxy_url = format!("http://127.0.0.1:{}", engine.mixed_port);

    // 1. Parallel Multi-Stream Download: 6 concurrent connections
    let parallel_streams = 6;
    let t_par_start = Instant::now();
    let handles: Vec<_> = (0..parallel_streams)
        .map(|_| {
            let p_url = proxy_url.clone();
            std::thread::spawn(move || {
                let client = Client::builder()
                    .proxy(reqwest::Proxy::all(&p_url).unwrap())
                    .timeout(Duration::from_secs(6))
                    .build()
                    .unwrap();

                // Download small asset from gstatic
                client
                    .get("https://www.gstatic.com/generate_204")
                    .send()
                    .is_ok()
            })
        })
        .collect();

    let mut par_success = 0;
    for h in handles {
        if h.join().unwrap_or(false) {
            par_success += 1;
        }
    }
    let parallel_download_total_ms = t_par_start.elapsed().as_millis() as f64;

    // 2. Multi-Component Web Page Fetch: 12 components across multiple distinct domains
    let multi_domains = [
        "https://www.gstatic.com/generate_204",
        "https://www.google.com/favicon.ico",
        "https://fonts.googleapis.com/css?family=Roboto",
        "https://ajax.googleapis.com/ajax/libs/jquery/3.6.0/jquery.min.js",
        "https://api.github.com/zen",
        "https://raw.githubusercontent.com/favicon.ico",
        "https://github.githubassets.com/favicons/favicon.png",
        "https://cloudflare.com/cdn-cgi/trace",
        "https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css",
        "https://notion.so/favicon.ico",
        "https://httpbin.org/status/200",
        "https://httpbin.org/headers",
    ];

    let t_page_start = Instant::now();
    let mut component_latencies = Vec::new();
    let client = Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url).unwrap())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let mut succ = 0;
    let mut fail = 0;

    for url in &multi_domains {
        let t0 = Instant::now();
        match client.get(*url).send() {
            Ok(resp) if resp.status().is_success() => {
                succ += 1;
                component_latencies.push(t0.elapsed().as_millis() as f64);
            }
            _ => {
                fail += 1;
                component_latencies.push(3000.0);
            }
        }
    }
    let page_load_total_ms = t_page_start.elapsed().as_millis() as f64;

    component_latencies.sort_by(|a, b| a.total_cmp(b));
    let p95_idx =
        ((component_latencies.len() as f64 * 0.95) as usize).min(component_latencies.len() - 1);
    let page_component_p95_ms = component_latencies[p95_idx];

    // 3. Egress IP Stability on same domain: 15 sequential requests to cloudflare trace
    let mut observed_ips = Vec::new();
    for _ in 0..15 {
        if let Ok(resp) = client.get("https://cloudflare.com/cdn-cgi/trace").send() {
            if let Ok(text) = resp.text() {
                for line in text.lines() {
                    if let Some(ip) = line.strip_prefix("ip=") {
                        observed_ips.push(ip.to_string());
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }

    let mut ip_changes = 0;
    for i in 1..observed_ips.len() {
        if observed_ips[i] != observed_ips[i - 1] {
            ip_changes += 1;
        }
    }

    MultiPathResults {
        strategy_name,
        parallel_download_total_ms,
        page_load_total_ms,
        page_component_p95_ms,
        ip_changes_on_same_domain: ip_changes,
        successful_requests: succ + par_success,
        failed_requests: fail,
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("MULTIPATH EGRESS & LOAD-BALANCE BENCHMARK");
    println!("Comparing: Single Best Server vs Round-Robin vs Consistent-Hashing");
    println!("Physical Endpoints: NL #1 (10 Gbit), Germany #2, Sweden #4 (10 Gbit)");
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

    let pool: Vec<Node> = settings
        .nodes
        .into_iter()
        .filter(|n| {
            (n.name.contains("NL #1")
                || n.name.contains("Germany #2")
                || n.name.contains("Sweden #4"))
                && n.enabled
        })
        .take(3)
        .collect();

    println!("Cluster nodes configured (3 distinct servers):");
    for n in &pool {
        println!("  - {}", n.name);
    }

    let mut all_results = Vec::new();

    // 1. Single Best Server (Smart Mode: NL #1 only)
    println!("\nTesting Mode 1: Single Best Node (Smart Mode - NL #1 only)...");
    let nl_only = vec![pool[0].clone()];
    let eng_single = start_mihomo_with_group(&nl_only, "url-test", None).expect("Failed single");
    let res_single = benchmark_engine("Single Best (NL #1)", &eng_single);
    println!(
        "  -> Parallel 6-stream: {:.1}ms | Page Load: {:.1}ms | IP Flaps on same domain: {}",
        res_single.parallel_download_total_ms,
        res_single.page_load_total_ms,
        res_single.ip_changes_on_same_domain
    );
    drop(eng_single);
    all_results.push(res_single);

    // 2. Load Balance: Round-Robin across all 3 nodes
    println!("\nTesting Mode 2: Load-Balance Round-Robin (3 nodes)...");
    let eng_rr =
        start_mihomo_with_group(&pool, "load-balance", Some("round-robin")).expect("Failed RR");
    let res_rr = benchmark_engine("Load Balance Round-Robin", &eng_rr);
    println!(
        "  -> Parallel 6-stream: {:.1}ms | Page Load: {:.1}ms | IP Flaps on same domain: {}",
        res_rr.parallel_download_total_ms,
        res_rr.page_load_total_ms,
        res_rr.ip_changes_on_same_domain
    );
    drop(eng_rr);
    all_results.push(res_rr);

    // 3. Load Balance: Consistent-Hashing across all 3 nodes
    println!("\nTesting Mode 3: Load-Balance Consistent-Hashing (3 nodes)...");
    let eng_ch = start_mihomo_with_group(&pool, "load-balance", Some("consistent-hashing"))
        .expect("Failed CH");
    let res_ch = benchmark_engine("Consistent Hashing", &eng_ch);
    println!(
        "  -> Parallel 6-stream: {:.1}ms | Page Load: {:.1}ms | IP Flaps on same domain: {}",
        res_ch.parallel_download_total_ms,
        res_ch.page_load_total_ms,
        res_ch.ip_changes_on_same_domain
    );
    drop(eng_ch);
    all_results.push(res_ch);

    // Display Comparative Table
    println!("\n=========================================================================================");
    println!("COMPARATIVE MULTIPATH RESULTS ON PHYSICAL NETWORK");
    println!(
        "========================================================================================="
    );
    println!(
        "{:<28} {:>16} {:>16} {:>14} {:>16}",
        "Strategy", "6-Parallel Stream", "12-Component Page", "Page p95", "Egress IP Flaps"
    );
    println!(
        "-----------------------------------------------------------------------------------------"
    );

    for r in &all_results {
        println!(
            "{:<28} {:>14.1}ms {:>14.1}ms {:>12.1}ms {:>16}",
            r.strategy_name,
            r.parallel_download_total_ms,
            r.page_load_total_ms,
            r.page_component_p95_ms,
            r.ip_changes_on_same_domain
        );
    }
    println!(
        "========================================================================================="
    );
}
