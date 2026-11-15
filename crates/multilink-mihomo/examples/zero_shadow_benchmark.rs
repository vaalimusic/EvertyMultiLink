//! ZERO Shadow Mode Benchmark for MultiLink
//! Evaluates MultiLink Smart Mode vs ZERO Multidim vs ZERO Dual Health
//! across realistic network events:
//! 1. Normal traffic
//! 2. Contextual censorship (node blocked on specific domain only)
//! 3. Total node outage (server crashes)
//! 4. Unannounced recovery
//! 5. Flaky network jitter spike
//! 6. Local client network failure (all nodes fail simultaneously)

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
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + (self.next() % (hi - lo + 1))
    }
    pub fn next_f64(&mut self) -> f64 {
        (self.next() & 0x001F_FFFF_FFFF_FFFF) as f64 / (0x0020_0000_0000_0000u64 as f64)
    }
}

#[derive(Clone, Debug)]
pub struct NetworkEventState {
    pub phase: usize,
    pub node_a_contextual_block: bool, // Node A blocked on github.com
    pub node_b_total_outage: bool,     // Node B 100% down
    pub node_c_jitter_spike: bool,     // Node C latency jumps to 350ms
    pub local_network_down: bool,      // Wi-Fi down: all fail
}

impl NetworkEventState {
    pub fn for_step(step: usize) -> Self {
        match step {
            0..=499 => Self {
                phase: 1, // Normal
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_down: false,
            },
            500..=999 => Self {
                phase: 2, // Contextual Censorship: Node A blocked on github.com
                node_a_contextual_block: true,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_down: false,
            },
            1000..=1499 => Self {
                phase: 3, // Total Outage: Node B crashes
                node_a_contextual_block: false,
                node_b_total_outage: true,
                node_c_jitter_spike: false,
                local_network_down: false,
            },
            1500..=1999 => Self {
                phase: 4, // Jitter Spike: Node C degrades
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: true,
                local_network_down: false,
            },
            2000..=2499 => Self {
                phase: 5, // Local Network Failure: transient drop
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_down: (step % 20) < 4, // 20% drops
            },
            _ => Self {
                phase: 6, // Recovery: all healthy
                node_a_contextual_block: false,
                node_b_total_outage: false,
                node_c_jitter_spike: false,
                local_network_down: false,
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

/// Simulates actual server connection attempt under the current environment.
fn simulate_connection(
    node: &Node,
    domain: &str,
    env: &NetworkEventState,
    rng: &mut Rng,
) -> (Option<u64>, bool) {
    if env.local_network_down {
        return (None, false); // Local connection failure
    }

    if node.name == "Node-A" && env.node_a_contextual_block && domain.contains("github.com") {
        return (None, false); // Contextual censorship failure
    }

    if node.name == "Node-B" && env.node_b_total_outage {
        return (None, false); // Total server outage
    }

    let (base_lat, jitter) = if node.name == "Node-C" && env.node_c_jitter_spike {
        (350.0, 80.0)
    } else {
        (node.quality.latency_ms, node.quality.jitter_ms)
    };

    // Stochastically apply node reliability
    if rng.next_f64() > node.quality.reliability {
        return (None, false);
    }

    let lat_noise = (rng.next_f64() * 2.0 - 1.0) * jitter;
    let actual_lat = (base_lat + lat_noise).max(5.0) as u64;
    (Some(actual_lat), true)
}

/// Simulated MultiLink Smart Mode selection:
/// Selects based on highest NodeQuality score with tolerance window and url-test switching.
fn smart_mode_select(
    nodes: &[Node],
    domain: &str,
    recent_url_test_fails: &HashMap<(String, String), u32>,
) -> String {
    let domain_key = domain.to_string();
    let mut best_score = -1.0;
    let mut best_name = nodes[0].name.clone();

    for node in nodes {
        let fails = recent_url_test_fails
            .get(&(node.name.clone(), domain_key.clone()))
            .copied()
            .unwrap_or(0);

        if fails >= 2 {
            continue; // url-test excluded this node for this site
        }

        let score = node.quality.score().unwrap_or(50) as f64;
        if score > best_score {
            best_score = score;
            best_name = node.name.clone();
        }
    }
    best_name
}

#[derive(Default, Clone)]
struct RunStats {
    total: usize,
    first_success: usize,
    reconnections: usize,
    latencies: Vec<u64>,
    agreements: usize,
    divergences: usize,
    divergence_zero_better: usize,
    divergence_mihomo_better: usize,
    decision_nanos: u128,
}

impl RunStats {
    fn record(&mut self, success: bool, latency: Option<u64>, nanos: u128) {
        self.total += 1;
        self.decision_nanos += nanos;
        if success {
            self.first_success += 1;
            if let Some(l) = latency {
                self.latencies.push(l);
            }
        } else {
            self.reconnections += 1;
            self.latencies.push(1200); // 1200ms penalty for reconnection
        }
    }

    fn mean_lat(&self) -> f64 {
        if self.latencies.is_empty() {
            0.0
        } else {
            self.latencies.iter().sum::<u64>() as f64 / self.latencies.len() as f64
        }
    }

    fn p50_p95(&mut self) -> (u64, u64) {
        if self.latencies.is_empty() {
            return (0, 0);
        }
        self.latencies.sort_unstable();
        let idx50 = ((self.latencies.len() as f64 * 0.50) as usize).min(self.latencies.len() - 1);
        let idx95 = ((self.latencies.len() as f64 * 0.95) as usize).min(self.latencies.len() - 1);
        (self.latencies[idx50], self.latencies[idx95])
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("EVERTY MULTILINK vs ZERO SHADOW MODE BENCHMARK");
    println!("Comparing: 1) Native Smart Mode  2) ZERO Multidim  3) ZERO Dual Health");
    println!("30 Independent Evaluation Seeds | 3,000 Tasks per Seed (90,000 total connections)");
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

    let mut smart_stats = RunStats::default();
    let mut zero_raw_stats = RunStats::default();
    let mut zero_dual_stats = RunStats::default();

    let seeds = 30;

    for s in 0..seeds {
        let seed = 20281001u64 + (s as u64) * 7919;
        let mut rng = Rng::new(seed);

        // Candidate pool:
        // Node A: Fast (45ms), low jitter, 98% rel. (Target of contextual censorship in P2)
        // Node B: Medium (95ms), solid, 99% rel. (Target of total outage in P3)
        // Node C: Moderate (130ms), 97% rel. (Target of jitter spike in P4)
        // Node D: Backup (190ms), high reliability 99.5%.
        let mut nodes = vec![
            create_node("Node-A", 45.0, 12.0, 0.98),
            create_node("Node-B", 95.0, 10.0, 0.99),
            create_node("Node-C", 130.0, 15.0, 0.97),
            create_node("Node-D", 190.0, 8.0, 0.995),
        ];

        let mut zero_dual = ZeroShadowMode::new(true);
        let mut zero_raw = ZeroShadowMode::new(true); // without dual health updates
        let mut recent_url_test_fails: HashMap<(String, String), u32> = HashMap::new();

        for step in 0..3000 {
            let env = NetworkEventState::for_step(step);
            let domain = domains[(rng.next() as usize) % domains.len()];

            // 1. Mihomo Smart Mode Choice
            let t0 = Instant::now();
            let mihomo_choice =
                black_box(smart_mode_select(&nodes, domain, &recent_url_test_fails));
            let smart_nanos = t0.elapsed().as_nanos();

            // 2. ZERO Recommendations
            let candidate_refs: Vec<&Node> = nodes.iter().collect();

            let t1 = Instant::now();
            let rec_raw = zero_raw.recommend(domain, &candidate_refs).unwrap();
            let raw_nanos = t1.elapsed().as_nanos();

            let t2 = Instant::now();
            let rec_dual = zero_dual.recommend(domain, &candidate_refs).unwrap();
            let dual_nanos = t2.elapsed().as_nanos();

            // Simulate actual outcomes for each selection
            let mut node_map: HashMap<&str, &Node> = HashMap::new();
            for n in &nodes {
                node_map.insert(&n.name, n);
            }

            let (m_lat, m_ok) =
                simulate_connection(node_map[&mihomo_choice.as_str()], domain, &env, &mut rng);
            let (raw_lat, raw_ok) = simulate_connection(
                node_map[rec_raw.recommended_server.as_str()],
                domain,
                &env,
                &mut rng,
            );
            let (dual_lat, dual_ok) = simulate_connection(
                node_map[rec_dual.recommended_server.as_str()],
                domain,
                &env,
                &mut rng,
            );

            smart_stats.record(m_ok, m_lat, smart_nanos);
            zero_raw_stats.record(raw_ok, raw_lat, raw_nanos);
            zero_dual_stats.record(dual_ok, dual_lat, dual_nanos);

            // Track agreement / divergence between Smart Mode and ZERO Dual
            if mihomo_choice == rec_dual.recommended_server {
                zero_dual_stats.agreements += 1;
            } else {
                zero_dual_stats.divergences += 1;
                let m_cost = m_lat.unwrap_or(1200);
                let d_cost = dual_lat.unwrap_or(1200);
                if d_cost < m_cost {
                    zero_dual_stats.divergence_zero_better += 1;
                } else if m_cost < d_cost {
                    zero_dual_stats.divergence_mihomo_better += 1;
                }
            }

            // Update Mihomo url-test state (replicates url-test max-failed-times: 2)
            let fail_key = (mihomo_choice.clone(), domain.to_string());
            if m_ok {
                recent_url_test_fails.insert(fail_key, 0);
            } else {
                let cnt = recent_url_test_fails.entry(fail_key).or_insert(0);
                *cnt += 1;
            }

            // Update ZERO Dual Health (only based on observed connection feedback!)
            zero_dual.observe_connection_feedback(&mihomo_choice, domain, m_ok);

            // Periodic NodeQuality EMA updates (from Mihomo /delays polling)
            if step % 15 == 0 {
                for node in &mut nodes {
                    let (del, ok) = simulate_connection(node, domain, &env, &mut rng);
                    node.quality.observe("sample", if ok { del } else { None });
                }
            }
        }

        if (s + 1) % 10 == 0 || s == seeds - 1 {
            println!(
                "Seed {:>2}/{} completed | Mihomo Smart={:.1}ms (ok={:.1}%) | ZERO Dual={:.1}ms (ok={:.1}%)",
                s + 1,
                seeds,
                smart_stats.mean_lat(),
                100.0 * smart_stats.first_success as f64 / smart_stats.total as f64,
                zero_dual_stats.mean_lat(),
                100.0 * zero_dual_stats.first_success as f64 / zero_dual_stats.total as f64,
            );
        }
    }

    let (s_p50, s_p95) = smart_stats.p50_p95();
    let (raw_p50, raw_p95) = zero_raw_stats.p50_p95();
    let (d_p50, d_p95) = zero_dual_stats.p50_p95();

    println!("\n=========================================================================================");
    println!("FINAL COMPARATIVE RESULTS (30 SEEDS, 90,000 PAIRED CONNECTIONS)");
    println!(
        "========================================================================================="
    );
    println!("Strategy                 Mean RTT   p50 RTT   p95 RTT   Success%   Reconnections  Latency(ns)");
    println!(
        "-----------------------------------------------------------------------------------------"
    );
    println!(
        "{:<24} {:>7.1}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>11}",
        "Current Smart Mode",
        smart_stats.mean_lat(),
        s_p50,
        s_p95,
        100.0 * smart_stats.first_success as f64 / smart_stats.total as f64,
        smart_stats.reconnections,
        smart_stats.decision_nanos / smart_stats.total as u128
    );
    println!(
        "{:<24} {:>7.1}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>11}",
        "ZERO Multidim (No Dual)",
        zero_raw_stats.mean_lat(),
        raw_p50,
        raw_p95,
        100.0 * zero_raw_stats.first_success as f64 / zero_raw_stats.total as f64,
        zero_raw_stats.reconnections,
        zero_raw_stats.decision_nanos / zero_raw_stats.total as u128
    );
    println!(
        "{:<24} {:>7.1}ms {:>7}ms {:>7}ms {:>9.2}% {:>14} {:>11}",
        "ZERO + Dual Health",
        zero_dual_stats.mean_lat(),
        d_p50,
        d_p95,
        100.0 * zero_dual_stats.first_success as f64 / zero_dual_stats.total as f64,
        zero_dual_stats.reconnections,
        zero_dual_stats.decision_nanos / zero_dual_stats.total as u128
    );
    println!(
        "========================================================================================="
    );

    println!("\nDIVERGENCE & VERIFICATION ANALYSIS (ZERO Dual Health vs Native Smart Mode):");
    println!("- Total Decisions: {}", zero_dual_stats.total);
    println!(
        "- Agreement with Smart Mode: {} ({:.1}%)",
        zero_dual_stats.agreements,
        100.0 * zero_dual_stats.agreements as f64 / zero_dual_stats.total as f64
    );
    println!(
        "- Divergence from Smart Mode: {} ({:.1}%)",
        zero_dual_stats.divergences,
        100.0 * zero_dual_stats.divergences as f64 / zero_dual_stats.total as f64
    );
    println!(
        "- In Divergences, ZERO proved better: {} times ({:.1}%)",
        zero_dual_stats.divergence_zero_better,
        100.0 * zero_dual_stats.divergence_zero_better as f64
            / zero_dual_stats.divergences.max(1) as f64
    );
    println!(
        "- In Divergences, Smart Mode proved better: {} times ({:.1}%)",
        zero_dual_stats.divergence_mihomo_better,
        100.0 * zero_dual_stats.divergence_mihomo_better as f64
            / zero_dual_stats.divergences.max(1) as f64
    );
}
