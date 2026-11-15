//! ZERO Recovery-Aware Exploration Benchmark (Stage 8 Extended)
//!
//! Investigates:
//! 1. Can ZERO detect node recovery WITHOUT spending excessive probe budget?
//! 2. Does Value-of-Information with Exponential Backoff beat Simple Adaptive & Fixed Interval?
//! 3. 4 Realistic Network Scenarios:
//!    - Scenario 1: Prolonged Outage (Node 0 down for 250 steps)
//!    - Scenario 2: Quick Flap (Node 0 down for 25 steps)
//!    - Scenario 3: Intermittent Flapping (Node 1 with 30% packet loss & spikes)
//!    - Scenario 4: Cascading Double Outage (Node 0 and Node 1 down simultaneously)
//!
//! 30 Independent Seeds per Scenario | Strictly Zero Lookahead

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

#[derive(Clone, Copy, Debug)]
pub struct SimNode {
    pub base_latency: f64,
    pub jitter: f64,
    pub alive: bool,
    pub packet_loss: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScenarioKind {
    ProlongedOutage,
    QuickFlap,
    IntermittentFlapping,
    CascadingDoubleOutage,
}

pub struct MultiScenarioWorld {
    pub step: usize,
    pub scenario: ScenarioKind,
    pub nodes: [SimNode; 4],
}

impl MultiScenarioWorld {
    pub fn new(scenario: ScenarioKind) -> Self {
        Self {
            step: 0,
            scenario,
            nodes: [
                SimNode {
                    base_latency: 45.0,
                    jitter: 6.0,
                    alive: true,
                    packet_loss: 0.0,
                }, // N0: Fast leader
                SimNode {
                    base_latency: 85.0,
                    jitter: 8.0,
                    alive: true,
                    packet_loss: 0.0,
                }, // N1: Medium
                SimNode {
                    base_latency: 125.0,
                    jitter: 10.0,
                    alive: true,
                    packet_loss: 0.0,
                }, // N2: Moderate
                SimNode {
                    base_latency: 180.0,
                    jitter: 5.0,
                    alive: true,
                    packet_loss: 0.0,
                }, // N3: Backup
            ],
        }
    }

    pub fn advance(&mut self, step: usize) {
        self.step = step;
        match self.scenario {
            ScenarioKind::ProlongedOutage => {
                // Node 0 down from 300 to 550 (250 steps)
                self.nodes[0].alive = !(300..550).contains(&step);
            }
            ScenarioKind::QuickFlap => {
                // Node 0 down from 300 to 325 (25 steps)
                self.nodes[0].alive = !(300..325).contains(&step);
            }
            ScenarioKind::IntermittentFlapping => {
                // Node 1 suffers 30% packet loss and jitter spikes from 200..700
                if (200..700).contains(&step) {
                    self.nodes[1].packet_loss = 0.30;
                    self.nodes[1].base_latency = 140.0;
                } else {
                    self.nodes[1].packet_loss = 0.0;
                    self.nodes[1].base_latency = 85.0;
                }
            }
            ScenarioKind::CascadingDoubleOutage => {
                // Node 0 down 200..600, Node 1 down 350..750 (both dead 350..600)
                self.nodes[0].alive = !(200..600).contains(&step);
                self.nodes[1].alive = !(350..750).contains(&step);
            }
        }
    }

    pub fn probe(&self, node_idx: usize, rng: &mut Rng) -> Option<f64> {
        let n = &self.nodes[node_idx];
        if !n.alive || rng.next_f64() < n.packet_loss {
            return None;
        }
        let noise = (rng.next_f64() * 2.0 - 1.0) * n.jitter;
        Some((n.base_latency + noise).max(5.0))
    }
}

#[derive(Default, Clone)]
pub struct RunStats {
    pub total_requests: usize,
    pub successful_requests: usize,
    pub experienced_rtts: Vec<f64>,
    pub probes_spent: usize,
    pub wasted_probes: usize,
    pub route_switches: usize,
    #[allow(dead_code)]
    pub recovery_delay_steps: usize,
}

impl RunStats {
    pub fn mean_rtt(&self) -> f64 {
        if self.experienced_rtts.is_empty() {
            0.0
        } else {
            self.experienced_rtts.iter().sum::<f64>() / self.experienced_rtts.len() as f64
        }
    }
    pub fn p95_rtt(&mut self) -> f64 {
        if self.experienced_rtts.is_empty() {
            return 0.0;
        }
        self.experienced_rtts.sort_by(|a, b| a.total_cmp(b));
        let idx = ((self.experienced_rtts.len() as f64 * 0.95) as usize)
            .min(self.experienced_rtts.len() - 1);
        self.experienced_rtts[idx]
    }
}

// -----------------------------------------------------------------------------
// Strategy 1: Fixed Interval Scheduler (Mihomo periodic url-test)
// -----------------------------------------------------------------------------
struct FixedScheduler {
    budget: usize,
    last_probe: usize,
    cached_rtt: [Option<f64>; 4],
    current_selected: usize,
}

impl FixedScheduler {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            last_probe: 0,
            cached_rtt: [Some(45.0), Some(85.0), Some(125.0), Some(180.0)],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &MultiScenarioWorld,
        rng: &mut Rng,
        stats: &mut RunStats,
    ) {
        // Probe all 4 nodes every 20 steps
        if step >= self.last_probe + 20 && self.budget >= 4 {
            self.last_probe = step;
            let mut changed = false;
            for i in 0..4 {
                let prev = self.cached_rtt[i];
                let res = world.probe(i, rng);
                stats.probes_spent += 1;
                self.budget -= 1;
                self.cached_rtt[i] = res;
                if prev == res {
                    stats.wasted_probes += 1;
                } else {
                    changed = true;
                }
            }
            if changed {
                let best = (0..4)
                    .filter(|&i| self.cached_rtt[i].is_some())
                    .min_by(|&a, &b| {
                        self.cached_rtt[a]
                            .unwrap()
                            .total_cmp(&self.cached_rtt[b].unwrap())
                    });
                if let Some(b) = best {
                    if b != self.current_selected {
                        self.current_selected = b;
                        stats.route_switches += 1;
                    }
                }
            }
        }

        // Serve user request
        stats.total_requests += 1;
        match world.probe(self.current_selected, rng) {
            Some(rtt) => {
                stats.successful_requests += 1;
                stats.experienced_rtts.push(rtt);
            }
            None => {
                stats.experienced_rtts.push(1200.0);
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Strategy 2: Simple Adaptive Heuristic
// -----------------------------------------------------------------------------
struct SimpleAdaptiveScheduler {
    budget: usize,
    last_probe: [usize; 4],
    cached_rtt: [Option<f64>; 4],
    current_selected: usize,
}

impl SimpleAdaptiveScheduler {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            last_probe: [0; 4],
            cached_rtt: [Some(45.0), Some(85.0), Some(125.0), Some(180.0)],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &MultiScenarioWorld,
        rng: &mut Rng,
        stats: &mut RunStats,
    ) {
        stats.total_requests += 1;
        let user_obs = world.probe(self.current_selected, rng);

        match user_obs {
            Some(rtt) => {
                stats.successful_requests += 1;
                stats.experienced_rtts.push(rtt);
                self.cached_rtt[self.current_selected] = Some(rtt);
                self.last_probe[self.current_selected] = step;
            }
            None => {
                stats.experienced_rtts.push(1200.0);
                self.cached_rtt[self.current_selected] = None;
                // Immediate failover probing: check candidates
                for i in 0..4 {
                    if i != self.current_selected && self.budget > 0 {
                        let res = world.probe(i, rng);
                        stats.probes_spent += 1;
                        self.budget -= 1;
                        self.cached_rtt[i] = res;
                        self.last_probe[i] = step;
                        if res.is_some() {
                            break;
                        }
                    }
                }
            }
        }

        // Background maintenance: re-check stale or dead nodes every 40 steps
        if self.budget > 0 {
            for i in 0..4 {
                if step.saturating_sub(self.last_probe[i]) >= 40 && self.budget > 0 {
                    let prev = self.cached_rtt[i];
                    let res = world.probe(i, rng);
                    stats.probes_spent += 1;
                    self.budget -= 1;
                    self.cached_rtt[i] = res;
                    self.last_probe[i] = step;
                    if prev == res {
                        stats.wasted_probes += 1;
                    }
                    break;
                }
            }
        }

        let best = (0..4)
            .filter(|&i| self.cached_rtt[i].is_some())
            .min_by(|&a, &b| {
                self.cached_rtt[a]
                    .unwrap()
                    .total_cmp(&self.cached_rtt[b].unwrap())
            })
            .unwrap_or(3);

        if best != self.current_selected {
            self.current_selected = best;
            stats.route_switches += 1;
        }
    }
}

// -----------------------------------------------------------------------------
// Strategy 3: ZERO Recovery-Aware Exploration Scheduler
// -----------------------------------------------------------------------------
struct ZeroRecoveryScheduler {
    budget: usize,
    historical_baseline: [f64; 4],
    belief_mean: [f64; 4],
    belief_var: [f64; 4],
    is_failing: [bool; 4],
    last_valid_step: [usize; 4],
    consecutive_probe_failures: [u32; 4],
    next_allowed_probe_step: [usize; 4],
    current_selected: usize,
}

impl ZeroRecoveryScheduler {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            historical_baseline: [45.0, 85.0, 125.0, 180.0],
            belief_mean: [45.0, 85.0, 125.0, 180.0],
            belief_var: [8.0, 8.0, 8.0, 8.0],
            is_failing: [false; 4],
            last_valid_step: [0; 4],
            consecutive_probe_failures: [0; 4],
            next_allowed_probe_step: [0; 4],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &MultiScenarioWorld,
        rng: &mut Rng,
        stats: &mut RunStats,
    ) {
        // 1. Serve user request
        stats.total_requests += 1;
        let chosen = self.current_selected;
        let obs = world.probe(chosen, rng);

        let active_failed = match obs {
            Some(rtt) => {
                stats.successful_requests += 1;
                stats.experienced_rtts.push(rtt);
                // Bayesian EMA update
                self.belief_mean[chosen] = 0.85 * self.belief_mean[chosen] + 0.15 * rtt;
                self.historical_baseline[chosen] = self.historical_baseline[chosen].min(rtt);
                self.belief_var[chosen] = 8.0;
                self.last_valid_step[chosen] = step;
                self.consecutive_probe_failures[chosen] = 0;
                self.is_failing[chosen] = false;
                false
            }
            None => {
                stats.experienced_rtts.push(1200.0);
                self.is_failing[chosen] = true;
                self.consecutive_probe_failures[chosen] += 1;
                // Exponential backoff before re-checking this failed node
                let backoff = 10 * (1 << self.consecutive_probe_failures[chosen].min(5));
                self.next_allowed_probe_step[chosen] = step + backoff;
                true
            }
        };

        // 2. Uncertainty propagation: unobserved nodes' variance grows with age
        for i in 0..4 {
            if i != chosen {
                let age = (step.saturating_sub(self.last_valid_step[i])) as f64;
                self.belief_var[i] = (self.belief_var[i] + 0.2 * age).min(300.0);
            }
        }

        // 3. Probing Decision (Emergency failover OR Recovery Exploration)
        if self.budget > 0 {
            if active_failed {
                // Emergency failover probe: test best viable candidate
                let mut candidates: Vec<usize> = (0..4)
                    .filter(|&i| i != chosen && !self.is_failing[i])
                    .collect();
                if candidates.is_empty() {
                    candidates = (0..4).filter(|&i| i != chosen).collect();
                }
                candidates.sort_by(|&a, &b| self.belief_mean[a].total_cmp(&self.belief_mean[b]));

                for &cand in &candidates {
                    if self.budget > 0 {
                        let res = world.probe(cand, rng);
                        stats.probes_spent += 1;
                        self.budget -= 1;

                        if let Some(rtt) = res {
                            self.belief_mean[cand] = rtt;
                            self.belief_var[cand] = 8.0;
                            self.last_valid_step[cand] = step;
                            self.consecutive_probe_failures[cand] = 0;
                            self.is_failing[cand] = false;
                            break;
                        } else {
                            self.is_failing[cand] = true;
                            self.consecutive_probe_failures[cand] += 1;
                            self.next_allowed_probe_step[cand] = step + 20;
                        }
                    }
                }
            } else {
                // RECOVERY-AWARE OPPORTUNISTIC PROBING:
                // Evaluates whether a currently inactive (or failed) node has a historical baseline
                // that is FASTER than our current leader!
                let active_latency = self.belief_mean[self.current_selected];
                let mut best_priority = 0.0;
                let mut target_candidate = None;

                for i in 0..4 {
                    if i != self.current_selected && step >= self.next_allowed_probe_step[i] {
                        let base = self.historical_baseline[i];
                        let potential_gain = active_latency - base;

                        if potential_gain > 15.0 {
                            // Recovery Probability increases with time since last probe
                            let age = step.saturating_sub(self.last_valid_step[i]) as f64;
                            let p_recovered = (1.0 - (-0.03 * age).exp()).max(0.2);
                            let expected_value = potential_gain * p_recovered;

                            if expected_value > best_priority {
                                best_priority = expected_value;
                                target_candidate = Some(i);
                            }
                        }
                    }
                }

                if let Some(t_cand) = target_candidate {
                    let prev_mean = self.belief_mean[t_cand];
                    let res = world.probe(t_cand, rng);
                    stats.probes_spent += 1;
                    self.budget -= 1;

                    if let Some(rtt) = res {
                        self.belief_mean[t_cand] = rtt;
                        self.belief_var[t_cand] = 8.0;
                        self.last_valid_step[t_cand] = step;
                        self.consecutive_probe_failures[t_cand] = 0;
                        self.is_failing[t_cand] = false;
                        if (rtt - prev_mean).abs() < 10.0 && !self.is_failing[t_cand] {
                            stats.wasted_probes += 1;
                        }
                    } else {
                        // Probe failed: apply exponential backoff!
                        self.consecutive_probe_failures[t_cand] += 1;
                        let backoff = 15 * (1 << self.consecutive_probe_failures[t_cand].min(5));
                        self.next_allowed_probe_step[t_cand] = step + backoff;
                    }
                }
            }
        }

        // 4. Update Leader
        let best = (0..4)
            .filter(|&i| !self.is_failing[i])
            .min_by(|&a, &b| self.belief_mean[a].total_cmp(&self.belief_mean[b]))
            .unwrap_or(3);

        if best != self.current_selected {
            self.current_selected = best;
            stats.route_switches += 1;
        }
    }
}

fn run_scenario_benchmark(scenario: ScenarioKind, label: &'static str) {
    println!("\n=========================================================================================");
    println!("SCENARIO: {}", label);
    println!("Comparing Fixed (Mihomo) vs Simple Adaptive vs ZERO Recovery-Aware (Budget = 250)");
    println!(
        "-----------------------------------------------------------------------------------------"
    );

    let seeds = 30;
    let mut fixed_stats: Vec<RunStats> = Vec::new();
    let mut simple_stats: Vec<RunStats> = Vec::new();
    let mut zero_stats: Vec<RunStats> = Vec::new();

    for s in 0..seeds {
        let seed = 2026110720u64 + (s as u64) * 6271;
        let mut rng = Rng::new(seed);

        let mut world = MultiScenarioWorld::new(scenario);
        let mut f_s = RunStats::default();
        let mut s_s = RunStats::default();
        let mut z_s = RunStats::default();

        let mut fixed = FixedScheduler::new(250);
        let mut simple = SimpleAdaptiveScheduler::new(250);
        let mut zero = ZeroRecoveryScheduler::new(250);

        for step in 0..1000 {
            world.advance(step);
            fixed.step(step, &world, &mut rng, &mut f_s);
            simple.step(step, &world, &mut rng, &mut s_s);
            zero.step(step, &world, &mut rng, &mut z_s);
        }

        fixed_stats.push(f_s);
        simple_stats.push(s_s);
        zero_stats.push(z_s);
    }

    println!(
        "{:<32} {:>10} {:>10} {:>10} {:>12} {:>12} {:>14}",
        "Strategy", "Mean RTT", "p95 RTT", "Success%", "Probes Used", "Wasted%", "Switches"
    );
    println!(
        "-----------------------------------------------------------------------------------------"
    );

    let print_row = |name: &'static str, list: Vec<RunStats>| {
        let n = list.len() as f64;
        let mean: f64 = list.iter().map(|s| s.mean_rtt()).sum::<f64>() / n;
        let mut all_rtts: Vec<f64> = list
            .iter()
            .flat_map(|s| s.experienced_rtts.clone())
            .collect();
        all_rtts.sort_by(|a, b| a.total_cmp(b));
        let p95 = all_rtts[(all_rtts.len() as f64 * 0.95) as usize];
        let tot_req: usize = list.iter().map(|s| s.total_requests).sum();
        let tot_succ: usize = list.iter().map(|s| s.successful_requests).sum();
        let succ_pct = 100.0 * tot_succ as f64 / tot_req as f64;
        let probes_avg: f64 = list.iter().map(|s| s.probes_spent).sum::<usize>() as f64 / n;
        let wasted_avg: f64 = list.iter().map(|s| s.wasted_probes).sum::<usize>() as f64 / n;
        let wasted_pct = if probes_avg > 0.0 {
            100.0 * wasted_avg / probes_avg
        } else {
            0.0
        };
        let sw_avg: f64 = list.iter().map(|s| s.route_switches).sum::<usize>() as f64 / n;

        println!(
            "{:<32} {:>9.1}ms {:>9.1}ms {:>9.2}% {:>12.0} {:>11.1}% {:>14.1}",
            name, mean, p95, succ_pct, probes_avg, wasted_pct, sw_avg
        );
    };

    print_row("Fixed Interval (Mihomo)", fixed_stats);
    print_row("Simple Adaptive Heuristic", simple_stats);
    print_row("ZERO Recovery-Aware Scheduler", zero_stats);
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("ZERO RECOVERY-AWARE EXPLORATION BENCHMARK (EXPERIMENT B)");
    println!("Testing Recovery Discovery, Exponential Backoff & Value of Information");
    println!(
        "========================================================================================="
    );

    run_scenario_benchmark(
        ScenarioKind::ProlongedOutage,
        "1. Prolonged Outage (Node 0 down 250 steps)",
    );
    run_scenario_benchmark(
        ScenarioKind::QuickFlap,
        "2. Quick Flap (Node 0 down 25 steps)",
    );
    run_scenario_benchmark(
        ScenarioKind::IntermittentFlapping,
        "3. Intermittent Packet Loss & Jitter Spikes",
    );
    run_scenario_benchmark(
        ScenarioKind::CascadingDoubleOutage,
        "4. Cascading Double Outage (Both fast nodes down)",
    );
}
