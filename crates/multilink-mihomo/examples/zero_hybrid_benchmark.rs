//! ZERO Hybrid Smart Mode Benchmark: L1 Availability + L2 Adaptive Scoring
//! Evaluates four distinct strategies under identical paired network environments:
//! 1. Native Smart Mode (Mihomo `url-test` with 100ms tolerance, interval 30-60s)
//! 2. Mihomo L1 + Min RTT with Hysteresis (20ms hysteresis baseline)
//! 3. Mihomo L1 + ZERO Multidim (Expected Cost Scoring)
//! 4. Mihomo L1 + ZERO Dual Health (Contextual + Global Health Filtering)
//!
//! Evaluates: Mean RTT, p50, p95, p99, first-attempt success %, reconnections,
//! background probe costs, decision latency, and behavior during outages & recovery.

use multilink_core::{Node, NodeQuality};
use multilink_mihomo::zero_shadow::ZeroShadowMode;
use serde_yaml::Mapping;
use std::collections::HashMap;
use std::hint::black_box;
use std::time::Instant;

pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn next_f64(&mut self) -> f64 {
        (self.next() & 0x001F_FFFF_FFFF_FFFF) as f64 / (0x0020_0000_0000_0000u64 as f64)
    }
}

#[derive(Clone, Debug)]
pub struct NetworkEnvironment {
    pub phase: usize,
    pub node_a_contextual_block: bool, // Node A blocked on github.com
    pub node_b_total_outage: bool,     // Node B crashed (100% fail)
    pub node_c_jitter_spike: bool,     // Node C latency jumps from 130ms to 320ms + jitter
    pub local_network_drop: bool,      // Wi-Fi packet drop (all nodes fail)
}

impl NetworkEnvironment {
    pub fn for_step(step: usize) -> Self {
        match step {
            0..=499 => Self {
                phase: 1, // Normal
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_drop: false,
            },
            500..=999 => Self {
                phase: 2, // Contextual censorship: Node A blocked on github.com only
                node_a_contextual_block: true,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_drop: false,
            },
            1000..=1499 => Self {
                phase: 3, // Complete outage: Node B server hardware failure
                node_a_contextual_block: false,
                node_b_total_outage: true,
                node_c_jitter_spike: false,
                local_network_drop: false,
            },
            1500..=1999 => Self {
                phase: 4, // Jitter & Congestion: Node C degrades heavily
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: true,
                local_network_drop: false,
            },
            2000..=2499 => Self {
                phase: 5, // Local network Wi-Fi instability (transient drops across all nodes)
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_drop: (step % 25) < 3, // ~12% intermittent local drops
            },
            _ => Self {
                phase: 6, // Unannounced full recovery
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_drop: false,
            },
        }
    }
}

fn create_node(name: &str, latency: f64, jitter: f64, reliability: f64) -> Node {
    let mut q = NodeQuality::default();
    q.latency_ms = latency;
    q.jitter_ms = jitter;
    q.reliability = reliability;
    q.samples = 20;
    Node {
        name: name.to_string(),
        enabled: true,
        proxy: Mapping::new(),
        spider_x: None,
        delay: Some(latency as u64),
        quality: q,
        subscription_ids: Vec::new(),
        manually_added: false,
    }
}

/// Simulates actual physical connection execution through a given node.
fn execute_connection(
    node: &Node,
    domain: &str,
    env: &NetworkEnvironment,
    rng: &mut Rng,
) -> (Option<u64>, bool) {
    if env.local_network_drop {
        return (None, false); // Local Wi-Fi dropped connection
    }

    if node.name == "Node-A" && env.node_a_contextual_block && domain.contains("github.com") {
        return (None, false); // Blocked contextually by target
    }

    if node.name == "Node-B" && env.node_b_total_outage {
        return (None, false); // Total server outage
    }

    let (base_lat, jitter) = if node.name == "Node-C" && env.node_c_jitter_spike {
        (320.0, 75.0)
    } else {
        (node.quality.latency_ms, node.quality.jitter_ms)
    };

    if rng.next_f64() > node.quality.reliability {
        return (None, false); // Stochastic packet loss / fail
    }

    let noise = (rng.next_f64() * 2.0 - 1.0) * jitter;
    let actual_rtt = (base_lat + noise).max(5.0) as u64;
    (Some(actual_rtt), true)
}

/// Simulated L1 Mihomo Health Tracker:
/// Runs background `url-test` checks at regular intervals.
/// Excludes nodes with consecutive failures >= 2.
#[derive(Clone, Default)]
struct MihomoL1Gate {
    fail_counts: HashMap<(String, String), u32>, // (node, domain) -> fail count
    global_fails: HashMap<String, u32>,          // node -> global fail count
}

impl MihomoL1Gate {
    /// Background L1 probe update (models Mihomo's periodic url-test probe)
    fn background_probe(
        &mut self,
        nodes: &[Node],
        domains: &[&str],
        env: &NetworkEnvironment,
        rng: &mut Rng,
    ) {
        for node in nodes {
            // General health probe (Google 204)
            let (_, ok_global) = execute_connection(node, "gstatic.com", env, rng);
            if ok_global {
                self.global_fails.insert(node.name.clone(), 0);
            } else {
                let cnt = self.global_fails.entry(node.name.clone()).or_insert(0);
                *cnt += 1;
            }

            // Domain-specific probe for Smart Site Groups
            for &d in domains {
                let (_, ok_site) = execute_connection(node, d, env, rng);
                let key = (node.name.clone(), d.to_string());
                if ok_site {
                    self.fail_counts.insert(key, 0);
                } else {
                    let cnt = self.fail_counts.entry(key).or_insert(0);
                    *cnt += 1;
                }
            }
        }
    }

    /// Filters nodes to return ONLY those confirmed alive and available by L1.
    fn filter_available<'a>(&self, nodes: &'a [Node], domain: &str) -> Vec<&'a Node> {
        nodes
            .iter()
            .filter(|n| {
                let g_fails = self.global_fails.get(&n.name).copied().unwrap_or(0);
                let s_fails = self
                    .fail_counts
                    .get(&(n.name.clone(), domain.to_string()))
                    .copied()
                    .unwrap_or(0);
                g_fails < 2 && s_fails < 2
            })
            .collect()
    }
}

// ---------------- Strategy 1: Native Smart Mode Selection ----------------
fn select_native_smart(available: &[&Node], last_selected: Option<&str>) -> String {
    if available.is_empty() {
        return "Node-D".to_string(); // Fallback
    }

    // Sort by score
    let mut sorted = available.to_vec();
    sorted.sort_by_key(|n| std::cmp::Reverse(n.quality.score().unwrap_or(50)));

    let best = sorted[0];

    // Hysteresis / tolerance: if previously selected node is still within 100ms tolerance, keep it
    if let Some(prev) = last_selected {
        if let Some(prev_node) = available.iter().find(|n| n.name == prev) {
            if (prev_node.quality.latency_ms - best.quality.latency_ms) <= 100.0 {
                return prev.to_string();
            }
        }
    }

    best.name.clone()
}

// ---------------- Strategy 2: L1 + Min RTT with Hysteresis (20ms) ----------------
fn select_min_rtt_hysteresis(available: &[&Node], last_selected: Option<&str>) -> String {
    if available.is_empty() {
        return "Node-D".to_string();
    }

    let best = available
        .iter()
        .min_by(|a, b| a.quality.latency_ms.total_cmp(&b.quality.latency_ms))
        .unwrap();

    const HYSTERESIS_MS: f64 = 20.0;
    if let Some(prev) = last_selected {
        if let Some(prev_node) = available.iter().find(|n| n.name == prev) {
            if (prev_node.quality.latency_ms - best.quality.latency_ms) <= HYSTERESIS_MS {
                return prev.to_string();
            }
        }
    }

    best.name.clone()
}

// ---------------- Performance Metrics Tracker ----------------
#[derive(Default, Clone)]
struct RunStats {
    total_connections: usize,
    first_success: usize,
    reconnections: usize,
    rtts: Vec<u64>,
    decision_nanos: u128,
}

impl RunStats {
    fn record(&mut self, success: bool, latency: Option<u64>, nanos: u128) {
        self.total_connections += 1;
        self.decision_nanos += nanos;
        if success {
            self.first_success += 1;
            if let Some(rtt) = latency {
                self.rtts.push(rtt);
            }
        } else {
            self.reconnections += 1;
            self.rtts.push(1200); // 1200ms penalty for failed connection & retry
        }
    }

    fn mean_rtt(&self) -> f64 {
        if self.rtts.is_empty() {
            0.0
        } else {
            self.rtts.iter().sum::<u64>() as f64 / self.rtts.len() as f64
        }
    }

    fn percentiles(&mut self) -> (u64, u64, u64) {
        if self.rtts.is_empty() {
            return (0, 0, 0);
        }
        self.rtts.sort_unstable();
        let len = self.rtts.len() as f64;
        let p50 = self.rtts[((len * 0.50) as usize).min(self.rtts.len() - 1)];
        let p95 = self.rtts[((len * 0.95) as usize).min(self.rtts.len() - 1)];
        let p99 = self.rtts[((len * 0.99) as usize).min(self.rtts.len() - 1)];
        (p50, p95, p99)
    }

    fn success_pct(&self) -> f64 {
        if self.total_connections == 0 {
            0.0
        } else {
            100.0 * self.first_success as f64 / self.total_connections as f64
        }
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("ZERO HYBRID SMART MODE BENCHMARK (L1 Availability Gate + L2 Adaptive Scoring)");
    println!(
        "Testing 4 Architectures across 30 Independent Seeds | 3,000 tasks/seed (90,000 requests)"
    );
    println!("Events: P1(Normal) -> P2(ContextBlock) -> P3(TotalOutage) -> P4(JitterSpike) -> P5(LocalDrop) -> P6(Recovery)");
    println!(
        "========================================================================================="
    );

    let domains = [
        "api.github.com",
        "raw.githubusercontent.com",
        "google.com",
        "youtube.com",
        "cloudflare.com",
        "notion.so",
    ];

    let mut stats_smart = RunStats::default();
    let mut stats_min_rtt = RunStats::default();
    let mut stats_zero_raw = RunStats::default();
    let mut stats_zero_dual = RunStats::default();

    let seeds = 30;

    for s in 0..seeds {
        let seed = 20282001u64 + (s as u64) * 7919;
        let mut rng = Rng::new(seed);

        // Nodes setup:
        // Node A: Fast (42ms), low jitter (8ms), 98% rel.
        // Node B: Medium (88ms), low jitter (6ms), 99% rel.
        // Node C: Moderate (125ms), jitter (14ms), 97% rel.
        // Node D: Stable backup (185ms), low jitter (5ms), 99.8% rel.
        let mut nodes = vec![
            create_node("Node-A", 42.0, 8.0, 0.98),
            create_node("Node-B", 88.0, 6.0, 0.99),
            create_node("Node-C", 125.0, 14.0, 0.97),
            create_node("Node-D", 185.0, 5.0, 0.998),
        ];

        let mut l1_gate = MihomoL1Gate::default();
        let mut zero_raw = ZeroShadowMode::new(true);
        let mut zero_dual = ZeroShadowMode::new(true);

        let mut last_smart_node = None;
        let mut last_min_rtt_node = None;

        for step in 0..3000 {
            let env = NetworkEnvironment::for_step(step);
            let domain = domains[(rng.next() as usize) % domains.len()];

            // L1 Periodic background url-test probing (every 15 steps ~ 15-30s in real time)
            if step % 15 == 0 {
                l1_gate.background_probe(&nodes, &domains, &env, &mut rng);
            }

            // L1 Availability Gate: filter candidates that are confirmed alive
            let alive_candidates = l1_gate.filter_available(&nodes, domain);
            let candidates: Vec<&Node> = if alive_candidates.is_empty() {
                nodes.iter().collect() // Emergency fallback to all enabled nodes
            } else {
                alive_candidates
            };

            // Build node map for simulation lookup
            let mut node_map: HashMap<&str, &Node> = HashMap::new();
            for n in &nodes {
                node_map.insert(&n.name, n);
            }

            // ---------------- 1. Strategy 1: Native Smart Mode ----------------
            let t0 = Instant::now();
            let choice_smart =
                black_box(select_native_smart(&candidates, last_smart_node.as_deref()));
            let ns_smart = t0.elapsed().as_nanos();
            last_smart_node = Some(choice_smart.clone());
            let (lat_smart, ok_smart) =
                execute_connection(node_map[choice_smart.as_str()], domain, &env, &mut rng);
            stats_smart.record(ok_smart, lat_smart, ns_smart);

            // ---------------- 2. Strategy 2: L1 + Min RTT with Hysteresis (20ms) ----------------
            let t1 = Instant::now();
            let choice_min_rtt = black_box(select_min_rtt_hysteresis(
                &candidates,
                last_min_rtt_node.as_deref(),
            ));
            let ns_min_rtt = t1.elapsed().as_nanos();
            last_min_rtt_node = Some(choice_min_rtt.clone());
            let (lat_min_rtt, ok_min_rtt) =
                execute_connection(node_map[choice_min_rtt.as_str()], domain, &env, &mut rng);
            stats_min_rtt.record(ok_min_rtt, lat_min_rtt, ns_min_rtt);

            // ---------------- 3. Strategy 3: L1 + ZERO Multidim (No Dual) ----------------
            let t2 = Instant::now();
            let rec_raw = zero_raw.recommend(domain, &candidates).unwrap();
            let ns_raw = t2.elapsed().as_nanos();
            let (lat_raw, ok_raw) = execute_connection(
                node_map[rec_raw.recommended_server.as_str()],
                domain,
                &env,
                &mut rng,
            );
            stats_zero_raw.record(ok_raw, lat_raw, ns_raw);

            // ---------------- 4. Strategy 4: L1 + ZERO Dual Health ----------------
            let t3 = Instant::now();
            let rec_dual = zero_dual.recommend(domain, &candidates).unwrap();
            let ns_dual = t3.elapsed().as_nanos();
            let (lat_dual, ok_dual) = execute_connection(
                node_map[rec_dual.recommended_server.as_str()],
                domain,
                &env,
                &mut rng,
            );
            stats_zero_dual.record(ok_dual, lat_dual, ns_dual);

            // Feedback update for ZERO Dual Health
            zero_dual.observe_connection_feedback(&rec_dual.recommended_server, domain, ok_dual);

            // Periodic NodeQuality EMA updates (every 20 steps)
            if step % 20 == 0 {
                for node in &mut nodes {
                    let (del, ok) = execute_connection(node, domain, &env, &mut rng);
                    node.quality.observe("sample", if ok { del } else { None });
                }
            }
        }

        if (s + 1) % 10 == 0 || s == seeds - 1 {
            println!(
                "Seed {:>2}/{} done | Smart={:.1}ms | MinRTT+Hyst={:.1}ms | L1+ZERO-Raw={:.1}ms | L1+ZERO-Dual={:.1}ms",
                s + 1,
                seeds,
                stats_smart.mean_rtt(),
                stats_min_rtt.mean_rtt(),
                stats_zero_raw.mean_rtt(),
                stats_zero_dual.mean_rtt(),
            );
        }
    }

    let (s_p50, s_p95, s_p99) = stats_smart.percentiles();
    let (m_p50, m_p95, m_p99) = stats_min_rtt.percentiles();
    let (raw_p50, raw_p95, raw_p99) = stats_zero_raw.percentiles();
    let (d_p50, d_p95, d_p99) = stats_zero_dual.percentiles();

    println!("\n=========================================================================================");
    println!("FINAL COMPARATIVE RESULTS (30 SEEDS, 90,000 PAIRED CONNECTIONS)");
    println!(
        "========================================================================================="
    );
    println!("Strategy                 Mean RTT   p50 RTT   p95 RTT   p99 RTT   Success%   Reconnections  Latency(ns)");
    println!(
        "-----------------------------------------------------------------------------------------"
    );
    println!(
        "{:<24} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>11}",
        "1. Current Smart Mode",
        stats_smart.mean_rtt(),
        s_p50,
        s_p95,
        s_p99,
        stats_smart.success_pct(),
        stats_smart.reconnections,
        stats_smart.decision_nanos / stats_smart.total_connections as u128
    );
    println!(
        "{:<24} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>11}",
        "2. L1 + Min RTT (Hyst)",
        stats_min_rtt.mean_rtt(),
        m_p50,
        m_p95,
        m_p99,
        stats_min_rtt.success_pct(),
        stats_min_rtt.reconnections,
        stats_min_rtt.decision_nanos / stats_min_rtt.total_connections as u128
    );
    println!(
        "{:<24} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>11}",
        "3. L1 + ZERO Multidim",
        stats_zero_raw.mean_rtt(),
        raw_p50,
        raw_p95,
        raw_p99,
        stats_zero_raw.success_pct(),
        stats_zero_raw.reconnections,
        stats_zero_raw.decision_nanos / stats_zero_raw.total_connections as u128
    );
    println!(
        "{:<24} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>11}",
        "4. L1 + ZERO Dual Health",
        stats_zero_dual.mean_rtt(),
        d_p50,
        d_p95,
        d_p99,
        stats_zero_dual.success_pct(),
        stats_zero_dual.reconnections,
        stats_zero_dual.decision_nanos / stats_zero_dual.total_connections as u128
    );
    println!(
        "========================================================================================="
    );
}
