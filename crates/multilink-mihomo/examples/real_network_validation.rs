//! MultiLink Smart Mode — Synthetic Network Validation Benchmark
//! (Monte Carlo simulation of network fluctuations, jitter, and censorship;
//! NOT live physical measurements against VPN servers).
//!
//! Tolerance Parameter Sweep (20ms, 30ms, 50ms, 100ms) vs ZERO Shadow Mode.
//!
//! IMPORTANT ARCHITECTURAL NOTE:
//! This benchmark is a controlled synthetic simulation designed to stress-test
//! hysteresis thresholds and observe mathematical failure modes under stochastic conditions.
//! For physical live-network measurements against actual VPN servers, see `real_server_benchmark.rs`.
//!
//! Investigates:
//! 1. Does reducing tolerance from 100ms to 20/30/50ms improve p50/p95 latency in simulation?
//! 2. Does lower tolerance cause route flapping (parasitic server switching) on long-lived sessions?
//! 3. How does tolerance affect failure recovery time and error rates?
//! 4. How do ZERO Shadow Mode recommendations compare across different tolerance settings?

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
pub struct NetworkEventState {
    pub phase: usize,
    pub node_a_contextual_block: bool,
    pub node_b_hardware_outage: bool,
    pub node_c_jitter_spike: bool,
    pub local_wifi_drop: bool,
}

impl NetworkEventState {
    pub fn for_step(step: usize) -> Self {
        match step {
            0..=499 => Self {
                phase: 1, // Normal traffic
                node_a_contextual_block: false,
                node_b_hardware_outage: false,
                node_c_jitter_spike: false,
                local_wifi_drop: false,
            },
            500..=999 => Self {
                phase: 2, // Contextual censorship on Node A (github.com only)
                node_a_contextual_block: true,
                node_b_hardware_outage: false,
                node_c_jitter_spike: false,
                local_wifi_drop: false,
            },
            1000..=1499 => Self {
                phase: 3, // Complete outage of Node B
                node_a_contextual_block: false,
                node_b_hardware_outage: true,
                node_c_jitter_spike: false,
                local_wifi_drop: false,
            },
            1500..=1999 => Self {
                phase: 4, // Jitter & packet loss spike on Node C
                node_a_contextual_block: false,
                node_b_hardware_outage: false,
                node_c_jitter_spike: true,
                local_wifi_drop: false,
            },
            2000..=2499 => Self {
                phase: 5, // Transient local Wi-Fi drop (~10% intermittent packet drops)
                node_a_contextual_block: false,
                node_b_hardware_outage: false,
                node_c_jitter_spike: false,
                local_wifi_drop: (step % 30) < 3,
            },
            _ => Self {
                phase: 6, // Unannounced full recovery
                node_a_contextual_block: false,
                node_b_hardware_outage: false,
                node_c_jitter_spike: false,
                local_wifi_drop: false,
            },
        }
    }
}

fn create_node(name: &str, latency: f64, jitter: f64, reliability: f64) -> Node {
    let mut q = NodeQuality::default();
    q.latency_ms = latency;
    q.jitter_ms = jitter;
    q.reliability = reliability;
    q.samples = 25;
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

fn execute_node_connection(
    node: &Node,
    domain: &str,
    env: &NetworkEventState,
    rng: &mut Rng,
) -> (Option<u64>, bool) {
    if env.local_wifi_drop {
        return (None, false);
    }
    if node.name == "Node-A" && env.node_a_contextual_block && domain.contains("github.com") {
        return (None, false);
    }
    if node.name == "Node-B" && env.node_b_hardware_outage {
        return (None, false);
    }

    let (base_lat, jitter) = if node.name == "Node-C" && env.node_c_jitter_spike {
        (330.0, 70.0)
    } else {
        (node.quality.latency_ms, node.quality.jitter_ms)
    };

    if rng.next_f64() > node.quality.reliability {
        return (None, false);
    }

    let noise = (rng.next_f64() * 2.0 - 1.0) * jitter;
    let actual_rtt = (base_lat + noise).max(5.0) as u64;
    (Some(actual_rtt), true)
}

/// Simulated Mihomo Smart Mode selection with parameterized `tolerance_ms`.
/// In Mihomo url-test: switches to candidate node only if:
/// `candidate_latency + tolerance <= current_latency`.
fn select_smart_mode_with_tolerance(
    available: &[&Node],
    current_selected: Option<&str>,
    tolerance_ms: f64,
) -> String {
    if available.is_empty() {
        return "Node-D".to_string();
    }

    // Find candidate with lowest latency
    let best = available
        .iter()
        .min_by(|a, b| a.quality.latency_ms.total_cmp(&b.quality.latency_ms))
        .unwrap();

    if let Some(cur_name) = current_selected {
        if let Some(cur_node) = available.iter().find(|n| n.name == cur_name) {
            // Mihomo tolerance rule: only switch if candidate is better by more than tolerance
            if cur_node.quality.latency_ms - best.quality.latency_ms <= tolerance_ms {
                return cur_name.to_string(); // Keep current node
            }
        }
    }

    best.name.clone()
}

#[derive(Default, Clone)]
struct ToleranceStats {
    tolerance_ms: u32,
    total_connections: usize,
    first_successes: usize,
    reconnections: usize,
    route_switches: usize, // Flapping / Churn count
    rtts: Vec<u64>,
    zero_agreements: usize,
    zero_divergences: usize,
    decision_nanos: u128,
}

impl ToleranceStats {
    fn record(
        &mut self,
        ok: bool,
        latency: Option<u64>,
        switched: bool,
        agreed_with_zero: bool,
        nanos: u128,
    ) {
        self.total_connections += 1;
        self.decision_nanos += nanos;
        if switched {
            self.route_switches += 1;
        }
        if agreed_with_zero {
            self.zero_agreements += 1;
        } else {
            self.zero_divergences += 1;
        }

        if ok {
            self.first_successes += 1;
            if let Some(rtt) = latency {
                self.rtts.push(rtt);
            }
        } else {
            self.reconnections += 1;
            self.rtts.push(1200); // 1200ms penalty for reconnection
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
            100.0 * self.first_successes as f64 / self.total_connections as f64
        }
    }

    fn switch_rate_pct(&self) -> f64 {
        if self.total_connections == 0 {
            0.0
        } else {
            100.0 * self.route_switches as f64 / self.total_connections as f64
        }
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("MULTILINK SMART MODE: SYNTHETIC NETWORK SIMULATION (MONTE CARLO)");
    println!("Note: This is a synthetic stress simulation under stochastic jitter/outages, NOT live VPN servers.");
    println!("Tolerances tested: [20ms, 30ms, 50ms, 100ms] vs ZERO Shadow Mode");
    println!("30 Independent Evaluation Seeds | 3,000 tasks/seed (90,000 requests per setting)");
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

    let tolerance_values = [20u32, 30u32, 50u32, 100u32];
    let mut all_stats: Vec<ToleranceStats> = tolerance_values
        .iter()
        .map(|&tol| ToleranceStats {
            tolerance_ms: tol,
            ..Default::default()
        })
        .collect();

    let seeds = 30;

    for s in 0..seeds {
        let seed = 20283001u64 + (s as u64) * 7919;
        let _rng = Rng::new(seed);

        for (tol_idx, &tolerance_ms) in tolerance_values.iter().enumerate() {
            let mut run_rng = Rng::new(seed ^ ((tol_idx as u64) << 24));

            // Candidate pool setup
            let mut nodes = vec![
                create_node("Node-A", 42.0, 10.0, 0.98),  // Fast, 10ms jitter
                create_node("Node-B", 85.0, 8.0, 0.99),   // Medium
                create_node("Node-C", 120.0, 12.0, 0.97), // Moderate
                create_node("Node-D", 180.0, 5.0, 0.998), // Backup
            ];

            let mut zero_shadow = ZeroShadowMode::new(true);
            let mut current_selected: Option<String> = None;
            let mut l1_fail_counts: HashMap<(String, String), u32> = HashMap::new();

            for step in 0..3000 {
                let env = NetworkEventState::for_step(step);
                let domain = domains[(run_rng.next() as usize) % domains.len()];

                // L1 periodic background health check (every 15 steps)
                if step % 15 == 0 {
                    for node in &nodes {
                        let (_, ok) = execute_node_connection(node, domain, &env, &mut run_rng);
                        let key = (node.name.clone(), domain.to_string());
                        if ok {
                            l1_fail_counts.insert(key, 0);
                        } else {
                            let cnt = l1_fail_counts.entry(key).or_insert(0);
                            *cnt += 1;
                        }
                    }
                }

                // L1 filter: available nodes
                let available: Vec<&Node> = nodes
                    .iter()
                    .filter(|n| {
                        l1_fail_counts
                            .get(&(n.name.clone(), domain.to_string()))
                            .copied()
                            .unwrap_or(0)
                            < 2
                    })
                    .collect();

                let candidates = if available.is_empty() {
                    nodes.iter().collect::<Vec<_>>()
                } else {
                    available
                };

                // 1. Mihomo Smart Mode selection with candidate tolerance
                let t0 = Instant::now();
                let chosen = black_box(select_smart_mode_with_tolerance(
                    &candidates,
                    current_selected.as_deref(),
                    tolerance_ms as f64,
                ));
                let decision_nanos = t0.elapsed().as_nanos();

                let switched = match &current_selected {
                    Some(prev) => prev != &chosen,
                    None => false,
                };
                current_selected = Some(chosen.clone());

                // 2. ZERO Shadow Recommendation
                let zero_rec = zero_shadow.recommend(domain, &candidates).unwrap();
                let agreed = zero_rec.recommended_server == chosen;

                // 3. Execution
                let chosen_node = nodes.iter().find(|n| n.name == chosen).unwrap();
                let (lat, ok) = execute_node_connection(chosen_node, domain, &env, &mut run_rng);

                all_stats[tol_idx].record(ok, lat, switched, agreed, decision_nanos);

                // ZERO Shadow passive update
                zero_shadow.observe_connection_feedback(&chosen, domain, ok);

                // Periodic NodeQuality EMA updates
                if step % 20 == 0 {
                    for node in &mut nodes {
                        let (del, ok) = execute_node_connection(node, domain, &env, &mut run_rng);
                        node.quality.observe("sample", if ok { del } else { None });
                    }
                }
            }
        }

        if (s + 1) % 10 == 0 || s == seeds - 1 {
            println!(
                "Seed {:>2}/{} completed | Tol=20ms: RTT={:.1}ms, Sw={:.1}% | Tol=100ms: RTT={:.1}ms, Sw={:.1}%",
                s + 1,
                seeds,
                all_stats[0].mean_rtt(),
                all_stats[0].switch_rate_pct(),
                all_stats[3].mean_rtt(),
                all_stats[3].switch_rate_pct(),
            );
        }
    }

    println!("\n=========================================================================================");
    println!("COMPARATIVE TOLERANCE SWEEP RESULTS (30 SEEDS, 90,000 REQUESTS PER SETTING)");
    println!(
        "========================================================================================="
    );
    println!("Tolerance    Mean RTT   p50 RTT   p95 RTT   p99 RTT   Success%   Reconnections  Flapping(Switches)   ZERO Agreement");
    println!("------------------------------------------------------------------------------------------------------------------");

    for stat in &mut all_stats {
        let (p50, p95, p99) = stat.percentiles();
        let agreement_pct = 100.0 * stat.zero_agreements as f64 / stat.total_connections as f64;
        println!(
            "{:<12} {:>7.1}ms {:>7}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>12} ({:>5.2}%) {:>15.1}%",
            format!("{} ms", stat.tolerance_ms),
            stat.mean_rtt(),
            p50,
            p95,
            p99,
            stat.success_pct(),
            stat.reconnections,
            stat.route_switches,
            stat.switch_rate_pct(),
            agreement_pct,
        );
    }
    println!(
        "========================================================================================="
    );
}
