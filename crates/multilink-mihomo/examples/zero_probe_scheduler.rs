//! ZERO Adaptive Probe Scheduler Benchmark
//! Rigorous comparison under identical Probe Budget (B = 200 probes per 1,000 steps)
//!
//! Strategies:
//! 1. Fixed Interval (Mihomo periodic: every 20 steps probes all 4 nodes)
//! 2. Simple Adaptive Heuristic (checks leader frequently, secondaries only when stale or on failure)
//! 3. ZERO Adaptive Probe Scheduler (Value of Information / POMDP belief tracking)
//! 4. Theoretical Oracle (upper bound, perfect ground truth)

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
pub struct GroundTruthNode {
    pub base_latency: f64,
    pub jitter: f64,
    pub alive: bool,
}

pub struct SimulationWorld {
    pub step: usize,
    pub nodes: [GroundTruthNode; 4],
}

impl SimulationWorld {
    pub fn new() -> Self {
        Self {
            step: 0,
            nodes: [
                GroundTruthNode {
                    base_latency: 45.0,
                    jitter: 8.0,
                    alive: true,
                }, // S0: Fast primary
                GroundTruthNode {
                    base_latency: 85.0,
                    jitter: 10.0,
                    alive: true,
                }, // S1: Medium
                GroundTruthNode {
                    base_latency: 125.0,
                    jitter: 12.0,
                    alive: true,
                }, // S2: Moderate
                GroundTruthNode {
                    base_latency: 180.0,
                    jitter: 5.0,
                    alive: true,
                }, // S3: Backup
            ],
        }
    }

    pub fn advance(&mut self, step: usize) {
        self.step = step;
        // Phase 1 (0..299): Normal
        // Phase 2 (300..549): Complete outage of primary Node 0
        // Phase 3 (550..699): Full recovery of Node 0
        // Phase 4 (700..849): Node 2 latency spike (degradation)
        // Phase 5 (850..999): Normal
        self.nodes[0].alive = !(300..550).contains(&step);
        if (700..850).contains(&step) {
            self.nodes[2].base_latency = 350.0;
        } else {
            self.nodes[2].base_latency = 125.0;
        }
    }

    pub fn probe(&self, node_idx: usize, rng: &mut Rng) -> Option<f64> {
        let n = &self.nodes[node_idx];
        if !n.alive {
            return None;
        }
        let noise = (rng.next_f64() * 2.0 - 1.0) * n.jitter;
        Some((n.base_latency + noise).max(5.0))
    }
}

#[derive(Default, Clone)]
pub struct SchedulerStats {
    pub total_user_requests: usize,
    pub successful_connections: usize,
    pub failed_connections: usize,
    pub experienced_rtts: Vec<f64>,
    pub probes_spent: usize,
    pub wasted_probes: usize, // probe that didn't change belief or routing
    pub outage_detection_delay_steps: usize,
    pub outage_recovery_delay_steps: usize,
    pub route_switches: usize,
}

impl SchedulerStats {
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
// Strategy 1: Fixed Interval Scheduler (Mihomo url-test baseline)
// -----------------------------------------------------------------------------
struct FixedIntervalScheduler {
    budget: usize,
    last_probe_step: usize,
    cached_latencies: [Option<f64>; 4],
    current_selected: usize,
}

impl FixedIntervalScheduler {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            last_probe_step: 0,
            cached_latencies: [Some(45.0), Some(85.0), Some(125.0), Some(180.0)],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &SimulationWorld,
        rng: &mut Rng,
        stats: &mut SchedulerStats,
    ) {
        // Probe all 4 nodes every 20 steps if budget permits
        if step >= self.last_probe_step + 20 && self.budget >= 4 {
            self.last_probe_step = step;
            let mut changed = false;
            for i in 0..4 {
                let res = world.probe(i, rng);
                stats.probes_spent += 1;
                self.budget -= 1;
                if self.cached_latencies[i] != res {
                    changed = true;
                }
                self.cached_latencies[i] = res;
            }
            if !changed {
                stats.wasted_probes += 4;
            }

            // Update selected
            let best = (0..4)
                .filter(|&i| self.cached_latencies[i].is_some())
                .min_by(|&a, &b| {
                    self.cached_latencies[a]
                        .unwrap()
                        .total_cmp(&self.cached_latencies[b].unwrap())
                });

            if let Some(b) = best {
                if b != self.current_selected {
                    self.current_selected = b;
                    stats.route_switches += 1;
                }
            }
        }

        // Serve user request
        let chosen = self.current_selected;
        stats.total_user_requests += 1;
        match world.probe(chosen, rng) {
            Some(rtt) => {
                stats.successful_connections += 1;
                stats.experienced_rtts.push(rtt);
            }
            None => {
                stats.failed_connections += 1;
                stats.experienced_rtts.push(1200.0); // 1200ms penalty
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Strategy 2: Simple Adaptive Heuristic Scheduler
// -----------------------------------------------------------------------------
struct HeuristicScheduler {
    budget: usize,
    last_checked: [usize; 4],
    cached_latencies: [Option<f64>; 4],
    current_selected: usize,
}

impl HeuristicScheduler {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            last_checked: [0; 4],
            cached_latencies: [Some(45.0), Some(85.0), Some(125.0), Some(180.0)],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &SimulationWorld,
        rng: &mut Rng,
        stats: &mut SchedulerStats,
    ) {
        // Serve user request through current selection
        let mut ok = false;
        let mut rtt_opt = None;
        stats.total_user_requests += 1;

        if let Some(rtt) = world.probe(self.current_selected, rng) {
            ok = true;
            rtt_opt = Some(rtt);
            stats.successful_connections += 1;
            stats.experienced_rtts.push(rtt);
            self.cached_latencies[self.current_selected] = Some(rtt);
            self.last_checked[self.current_selected] = step;
        } else {
            stats.failed_connections += 1;
            stats.experienced_rtts.push(1200.0);
            self.cached_latencies[self.current_selected] = None;
        }

        // Adaptive Probe Decision:
        // 1. If active node failed, immediately probe other nodes
        if !ok && self.budget > 0 {
            for i in 0..4 {
                if i != self.current_selected && self.budget > 0 {
                    let res = world.probe(i, rng);
                    stats.probes_spent += 1;
                    self.budget -= 1;
                    self.cached_latencies[i] = res;
                    self.last_checked[i] = step;
                }
            }
        } else if self.budget > 0 {
            // 2. Check stale nodes if age > 50 steps
            for i in 0..4 {
                if step.saturating_sub(self.last_checked[i]) >= 50 && self.budget > 0 {
                    let prev = self.cached_latencies[i];
                    let res = world.probe(i, rng);
                    stats.probes_spent += 1;
                    self.budget -= 1;
                    self.cached_latencies[i] = res;
                    self.last_checked[i] = step;
                    if prev == res {
                        stats.wasted_probes += 1;
                    }
                    break; // Max 1 maintenance probe per step
                }
            }
        }

        // Selection update
        let best = (0..4)
            .filter(|&i| self.cached_latencies[i].is_some())
            .min_by(|&a, &b| {
                self.cached_latencies[a]
                    .unwrap()
                    .total_cmp(&self.cached_latencies[b].unwrap())
            });

        if let Some(b) = best {
            if b != self.current_selected {
                self.current_selected = b;
                stats.route_switches += 1;
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Strategy 3: ZERO Adaptive Probe Scheduler (Value of Information / POMDP)
// -----------------------------------------------------------------------------
struct ZeroAdaptiveScheduler {
    budget: usize,
    belief_mean: [f64; 4],
    belief_var: [f64; 4],
    failure_score: [f64; 4],
    last_probe_step: [usize; 4],
    current_selected: usize,
}

impl ZeroAdaptiveScheduler {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            belief_mean: [45.0, 85.0, 125.0, 180.0],
            belief_var: [10.0, 10.0, 10.0, 10.0],
            failure_score: [0.0; 4],
            last_probe_step: [0; 4],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &SimulationWorld,
        rng: &mut Rng,
        stats: &mut SchedulerStats,
    ) {
        // 1. Serve user request through currently selected node
        stats.total_user_requests += 1;
        let chosen = self.current_selected;
        let user_obs = world.probe(chosen, rng);

        match user_obs {
            Some(rtt) => {
                stats.successful_connections += 1;
                stats.experienced_rtts.push(rtt);
                // Passive Bayesian update on observed node
                self.belief_mean[chosen] = 0.8 * self.belief_mean[chosen] + 0.2 * rtt;
                self.belief_var[chosen] = 10.0; // Uncertainty reset
                self.failure_score[chosen] *= 0.8;
                self.last_probe_step[chosen] = step;
            }
            None => {
                stats.failed_connections += 1;
                stats.experienced_rtts.push(1200.0);
                self.failure_score[chosen] = (self.failure_score[chosen] + 1.0).min(5.0);
                self.belief_mean[chosen] = 1200.0;
            }
        }

        // 2. Uncertainty propagation for unobserved nodes: variance grows with age
        for i in 0..4 {
            if i != chosen {
                let age = (step.saturating_sub(self.last_probe_step[i])) as f64;
                self.belief_var[i] += 0.5 * age; // Linear uncertainty accumulation
            }
        }

        // 3. ZERO Value of Information (VoI) Probing Decision
        if self.budget > 0 {
            let active_mean = self.belief_mean[self.current_selected];

            // If active node is failing, trigger emergency probe to the most promising candidate
            if self.failure_score[self.current_selected] >= 1.0 {
                let mut candidates: Vec<usize> =
                    (0..4).filter(|&i| i != self.current_selected).collect();
                candidates.sort_by(|&a, &b| self.belief_mean[a].total_cmp(&self.belief_mean[b]));

                for &cand in &candidates {
                    if self.budget > 0 {
                        let res = world.probe(cand, rng);
                        stats.probes_spent += 1;
                        self.budget -= 1;
                        self.last_probe_step[cand] = step;
                        if let Some(rtt) = res {
                            self.belief_mean[cand] = rtt;
                            self.belief_var[cand] = 10.0;
                            self.failure_score[cand] = 0.0;
                            break; // Found alive candidate
                        } else {
                            self.belief_mean[cand] = 1200.0;
                            self.failure_score[cand] = 5.0;
                        }
                    }
                }
            } else {
                // VoI opportunistic probe: check if an unselected node might have recovered or improved
                let mut best_voi = 0.0;
                let mut best_target = None;

                for i in 0..4 {
                    if i != self.current_selected {
                        // Expected value of checking node i:
                        // Upper confidence bound of potential gain: (active_mean - optimistic_i)
                        let optimistic_rtt =
                            self.belief_mean[i] - 1.2 * self.belief_var[i].sqrt().min(40.0);
                        let potential_gain = active_mean - optimistic_rtt;
                        if potential_gain > best_voi && potential_gain > 20.0 {
                            // Must beat switching threshold
                            best_voi = potential_gain;
                            best_target = Some(i);
                        }
                    }
                }

                if let Some(target) = best_target {
                    let prev_mean = self.belief_mean[target];
                    let res = world.probe(target, rng);
                    stats.probes_spent += 1;
                    self.budget -= 1;
                    self.last_probe_step[target] = step;

                    if let Some(rtt) = res {
                        self.belief_mean[target] = rtt;
                        self.belief_var[target] = 10.0;
                        self.failure_score[target] = 0.0;
                        if (rtt - prev_mean).abs() < 10.0 {
                            stats.wasted_probes += 1;
                        }
                    } else {
                        self.belief_mean[target] = 1200.0;
                        self.failure_score[target] = 5.0;
                    }
                }
            }
        }

        // 4. Update selected leader based on current belief
        let best = (0..4)
            .filter(|&i| self.failure_score[i] < 1.0)
            .min_by(|&a, &b| self.belief_mean[a].total_cmp(&self.belief_mean[b]))
            .unwrap_or(3); // fallback to backup

        if best != self.current_selected {
            self.current_selected = best;
            stats.route_switches += 1;
        }
    }
}

// -----------------------------------------------------------------------------
// Strategy 4: Theoretical Oracle (Omniscient Upper Bound)
// -----------------------------------------------------------------------------
struct OracleScheduler {
    current_selected: usize,
}

impl OracleScheduler {
    fn new() -> Self {
        Self {
            current_selected: 0,
        }
    }
    fn step(
        &mut self,
        _step: usize,
        world: &SimulationWorld,
        rng: &mut Rng,
        stats: &mut SchedulerStats,
    ) {
        // Oracle picks the truly best alive node without spending any probe budget
        let best = (0..4)
            .filter(|&i| world.nodes[i].alive)
            .min_by(|&a, &b| {
                world.nodes[a]
                    .base_latency
                    .total_cmp(&world.nodes[b].base_latency)
            })
            .unwrap_or(3);

        if best != self.current_selected {
            self.current_selected = best;
            stats.route_switches += 1;
        }

        stats.total_user_requests += 1;
        let rtt = world.probe(self.current_selected, rng).unwrap_or(1200.0);
        stats.successful_connections += 1;
        stats.experienced_rtts.push(rtt);
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("ZERO ADAPTIVE PROBE SCHEDULER BENCHMARK (STAGE 8)");
    println!("Equal Budget Constraint: B = 200 probes per 1,000 steps");
    println!(
        "30 Independent Evaluation Seeds | Outage Injection (300..550) & Degradation (700..850)"
    );
    println!("Zero Lookahead / Pure Posterior Belief");
    println!(
        "========================================================================================="
    );

    let seeds = 30;
    let mut fixed_all: Vec<SchedulerStats> = Vec::new();
    let mut heur_all: Vec<SchedulerStats> = Vec::new();
    let mut zero_all: Vec<SchedulerStats> = Vec::new();
    let mut oracle_all: Vec<SchedulerStats> = Vec::new();

    for s in 0..seeds {
        let seed = 2026110701u64 + (s as u64) * 8191;
        let mut rng = Rng::new(seed);

        let mut world = SimulationWorld::new();
        let mut fixed_stats = SchedulerStats::default();
        let mut heur_stats = SchedulerStats::default();
        let mut zero_stats = SchedulerStats::default();
        let mut oracle_stats = SchedulerStats::default();

        let mut fixed = FixedIntervalScheduler::new(200);
        let mut heur = HeuristicScheduler::new(200);
        let mut zero = ZeroAdaptiveScheduler::new(200);
        let mut oracle = OracleScheduler::new();

        for step in 0..1000 {
            world.advance(step);
            fixed.step(step, &world, &mut rng, &mut fixed_stats);
            heur.step(step, &world, &mut rng, &mut heur_stats);
            zero.step(step, &world, &mut rng, &mut zero_stats);
            oracle.step(step, &world, &mut rng, &mut oracle_stats);

            // Measure outage detection and recovery delays for seed 0
            if step == 301 {
                if fixed.current_selected == 0 {
                    fixed_stats.outage_detection_delay_steps += 1;
                }
                if heur.current_selected == 0 {
                    heur_stats.outage_detection_delay_steps += 1;
                }
                if zero.current_selected == 0 {
                    zero_stats.outage_detection_delay_steps += 1;
                }
            }
            if step >= 550 && step < 580 {
                if fixed.current_selected != 0 {
                    fixed_stats.outage_recovery_delay_steps += 1;
                }
                if heur.current_selected != 0 {
                    heur_stats.outage_recovery_delay_steps += 1;
                }
                if zero.current_selected != 0 {
                    zero_stats.outage_recovery_delay_steps += 1;
                }
            }
        }

        fixed_all.push(fixed_stats);
        heur_all.push(heur_stats);
        zero_all.push(zero_stats);
        oracle_all.push(oracle_stats);
    }

    println!("\n=========================================================================================");
    println!("COMPARATIVE PROBE SCHEDULING RESULTS (30 SEEDS, BUDGET = 200 PROBES)");
    println!(
        "========================================================================================="
    );
    println!(
        "{:<28} {:>10} {:>10} {:>10} {:>12} {:>12} {:>14}",
        "Strategy", "Mean RTT", "p95 RTT", "Success%", "Probes Used", "Wasted%", "Route Switches"
    );
    println!(
        "-----------------------------------------------------------------------------------------"
    );

    let summarize = |name: &'static str, mut list: Vec<SchedulerStats>| {
        let n = list.len() as f64;
        let mean_rtt: f64 = list.iter().map(|s| s.mean_rtt()).sum::<f64>() / n;
        let mut all_rtts: Vec<f64> = list
            .iter()
            .flat_map(|s| s.experienced_rtts.clone())
            .collect();
        all_rtts.sort_by(|a, b| a.total_cmp(b));
        let p95 = all_rtts[(all_rtts.len() as f64 * 0.95) as usize];
        let total_req: usize = list.iter().map(|s| s.total_user_requests).sum();
        let total_succ: usize = list.iter().map(|s| s.successful_connections).sum();
        let succ_pct = 100.0 * total_succ as f64 / total_req as f64;
        let probes_avg: f64 = list.iter().map(|s| s.probes_spent).sum::<usize>() as f64 / n;
        let wasted_avg: f64 = list.iter().map(|s| s.wasted_probes).sum::<usize>() as f64 / n;
        let wasted_pct = if probes_avg > 0.0 {
            100.0 * wasted_avg / probes_avg
        } else {
            0.0
        };
        let switches_avg: f64 = list.iter().map(|s| s.route_switches).sum::<usize>() as f64 / n;

        println!(
            "{:<28} {:>9.1}ms {:>9.1}ms {:>9.2}% {:>12.0} {:>11.1}% {:>14.1}",
            name, mean_rtt, p95, succ_pct, probes_avg, wasted_pct, switches_avg
        );
    };

    summarize("Fixed Interval (Mihomo)", fixed_all);
    summarize("Simple Adaptive Heuristic", heur_all);
    summarize("ZERO Adaptive Scheduler", zero_all);
    summarize("Theoretical Oracle", oracle_all);

    println!(
        "========================================================================================="
    );
}
