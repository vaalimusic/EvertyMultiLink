//! ZERO Probe Scheduling — Pareto Benchmark
//! Compares Cost (Probes / 1,000 steps) vs Performance (Mean User RTT)
//! Across:
//! 1. Fixed Interval (Mihomo periodic url-test)
//! 2. Simple Adaptive Heuristic (failure-triggered + stale timeout)
//! 3. ZERO Recovery-Aware Scheduler (POMDP belief + Value of Information + Exponential Backoff)
//!
//! Evaluated on Stochastic Hidden Markov Failures (unannounced multiple outages & recoveries)
//! 30 Seeds per configuration point | Strictly Zero Lookahead

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
pub struct MarkovNode {
    pub base_latency: f64,
    pub jitter: f64,
    pub alive: bool,
    pub p_fail: f64,
    pub p_recover: f64,
}

pub struct StochasticWorld {
    pub step: usize,
    pub nodes: [MarkovNode; 4],
    pub failure_events: Vec<(usize, usize)>,  // (node, step)
    pub recovery_events: Vec<(usize, usize)>, // (node, step)
}

impl StochasticWorld {
    pub fn new() -> Self {
        Self {
            step: 0,
            nodes: [
                MarkovNode {
                    base_latency: 45.0,
                    jitter: 6.0,
                    alive: true,
                    p_fail: 0.003,
                    p_recover: 0.02,
                }, // N0: Fast primary
                MarkovNode {
                    base_latency: 85.0,
                    jitter: 8.0,
                    alive: true,
                    p_fail: 0.002,
                    p_recover: 0.03,
                }, // N1: Medium
                MarkovNode {
                    base_latency: 125.0,
                    jitter: 10.0,
                    alive: true,
                    p_fail: 0.001,
                    p_recover: 0.04,
                }, // N2: Moderate
                MarkovNode {
                    base_latency: 180.0,
                    jitter: 5.0,
                    alive: true,
                    p_fail: 0.0001,
                    p_recover: 0.05,
                }, // N3: Backup
            ],
            failure_events: Vec::new(),
            recovery_events: Vec::new(),
        }
    }

    pub fn advance(&mut self, step: usize, rng: &mut Rng) {
        self.step = step;
        for i in 0..4 {
            let n = &mut self.nodes[i];
            if n.alive {
                if rng.next_f64() < n.p_fail {
                    n.alive = false;
                    self.failure_events.push((i, step));
                }
            } else if rng.next_f64() < n.p_recover {
                n.alive = true;
                self.recovery_events.push((i, step));
            }
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
pub struct ParetoMetrics {
    pub strategy_name: &'static str,
    pub param_label: String,
    pub actual_probes: f64,
    pub mean_rtt: f64,
    pub p95_rtt: f64,
    pub p99_rtt: f64,
    pub success_pct: f64,
    pub actionable_probes_pct: f64,
    #[allow(dead_code)]
    pub detection_delay_avg: f64,
    #[allow(dead_code)]
    pub recovery_delay_avg: f64,
    #[allow(dead_code)]
    pub route_switches: f64,
}

trait SchedTrait {
    fn step(
        &mut self,
        step: usize,
        world: &StochasticWorld,
        rng: &mut Rng,
        probes_spent: &mut usize,
        actionable_probes: &mut usize,
        switches: &mut usize,
        rtts: &mut Vec<f64>,
        succ: &mut usize,
    );
}

impl SchedTrait for FixedScheduler {
    fn step(
        &mut self,
        step: usize,
        world: &StochasticWorld,
        rng: &mut Rng,
        probes_spent: &mut usize,
        actionable_probes: &mut usize,
        switches: &mut usize,
        rtts: &mut Vec<f64>,
        succ: &mut usize,
    ) {
        self.step(
            step,
            world,
            rng,
            probes_spent,
            actionable_probes,
            switches,
            rtts,
            succ,
        );
    }
}

impl SchedTrait for SimpleAdaptiveScheduler {
    fn step(
        &mut self,
        step: usize,
        world: &StochasticWorld,
        rng: &mut Rng,
        probes_spent: &mut usize,
        actionable_probes: &mut usize,
        switches: &mut usize,
        rtts: &mut Vec<f64>,
        succ: &mut usize,
    ) {
        self.step(
            step,
            world,
            rng,
            probes_spent,
            actionable_probes,
            switches,
            rtts,
            succ,
        );
    }
}

impl SchedTrait for ZeroScheduler {
    fn step(
        &mut self,
        step: usize,
        world: &StochasticWorld,
        rng: &mut Rng,
        probes_spent: &mut usize,
        actionable_probes: &mut usize,
        switches: &mut usize,
        rtts: &mut Vec<f64>,
        succ: &mut usize,
    ) {
        self.step(
            step,
            world,
            rng,
            probes_spent,
            actionable_probes,
            switches,
            rtts,
            succ,
        );
    }
}
struct FixedScheduler {
    interval: usize,
    last_probe: usize,
    cached_rtt: [Option<f64>; 4],
    current_selected: usize,
}

impl FixedScheduler {
    fn new(interval: usize) -> Self {
        Self {
            interval,
            last_probe: 0,
            cached_rtt: [Some(45.0), Some(85.0), Some(125.0), Some(180.0)],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &StochasticWorld,
        rng: &mut Rng,
        probes_spent: &mut usize,
        actionable_probes: &mut usize,
        switches: &mut usize,
        rtts: &mut Vec<f64>,
        succ: &mut usize,
    ) {
        if step >= self.last_probe + self.interval {
            self.last_probe = step;
            let prev_selected = self.current_selected;
            for i in 0..4 {
                let res = world.probe(i, rng);
                *probes_spent += 1;
                self.cached_rtt[i] = res;
            }

            let best = (0..4)
                .filter(|&i| self.cached_rtt[i].is_some())
                .min_by(|&a, &b| {
                    self.cached_rtt[a]
                        .unwrap()
                        .total_cmp(&self.cached_rtt[b].unwrap())
                })
                .unwrap_or(3);

            if best != prev_selected {
                *actionable_probes += 4;
                *switches += 1;
                self.current_selected = best;
            }
        }

        // Serve user request
        match world.probe(self.current_selected, rng) {
            Some(rtt) => {
                *succ += 1;
                rtts.push(rtt);
            }
            None => {
                rtts.push(1200.0);
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Strategy 2: Simple Adaptive Heuristic Scheduler
// -----------------------------------------------------------------------------
struct SimpleAdaptiveScheduler {
    maint_interval: usize,
    last_probe: [usize; 4],
    cached_rtt: [Option<f64>; 4],
    current_selected: usize,
}

impl SimpleAdaptiveScheduler {
    fn new(maint_interval: usize) -> Self {
        Self {
            maint_interval,
            last_probe: [0; 4],
            cached_rtt: [Some(45.0), Some(85.0), Some(125.0), Some(180.0)],
            current_selected: 0,
        }
    }

    fn step(
        &mut self,
        step: usize,
        world: &StochasticWorld,
        rng: &mut Rng,
        probes_spent: &mut usize,
        actionable_probes: &mut usize,
        switches: &mut usize,
        rtts: &mut Vec<f64>,
        succ: &mut usize,
    ) {
        let user_obs = world.probe(self.current_selected, rng);

        match user_obs {
            Some(rtt) => {
                *succ += 1;
                rtts.push(rtt);
                self.cached_rtt[self.current_selected] = Some(rtt);
                self.last_probe[self.current_selected] = step;
            }
            None => {
                rtts.push(1200.0);
                self.cached_rtt[self.current_selected] = None;
                // Immediate failover probing
                for i in 0..4 {
                    if i != self.current_selected {
                        *probes_spent += 1;
                        let res = world.probe(i, rng);
                        self.cached_rtt[i] = res;
                        self.last_probe[i] = step;
                        if res.is_some() {
                            *actionable_probes += 1;
                            break;
                        }
                    }
                }
            }
        }

        // Stale node maintenance probing
        for i in 0..4 {
            if step.saturating_sub(self.last_probe[i]) >= self.maint_interval {
                *probes_spent += 1;
                let prev = self.cached_rtt[i];
                let res = world.probe(i, rng);
                self.cached_rtt[i] = res;
                self.last_probe[i] = step;
                if prev.is_none() && res.is_some() {
                    *actionable_probes += 1;
                }
                break;
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
            *switches += 1;
        }
    }
}

// -----------------------------------------------------------------------------
// Strategy 3: ZERO Recovery-Aware Scheduler
// -----------------------------------------------------------------------------
struct ZeroScheduler {
    gain_thresh: f64,
    base_backoff: usize,
    historical_baseline: [f64; 4],
    belief_mean: [f64; 4],
    belief_var: [f64; 4],
    is_failing: [bool; 4],
    last_valid_step: [usize; 4],
    consecutive_probe_failures: [u32; 4],
    next_allowed_probe_step: [usize; 4],
    current_selected: usize,
}

impl ZeroScheduler {
    fn new(gain_thresh: f64, base_backoff: usize) -> Self {
        Self {
            gain_thresh,
            base_backoff,
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
        world: &StochasticWorld,
        rng: &mut Rng,
        probes_spent: &mut usize,
        actionable_probes: &mut usize,
        switches: &mut usize,
        rtts: &mut Vec<f64>,
        succ: &mut usize,
    ) {
        let chosen = self.current_selected;
        let obs = world.probe(chosen, rng);

        let active_failed = match obs {
            Some(rtt) => {
                *succ += 1;
                rtts.push(rtt);
                self.belief_mean[chosen] = 0.85 * self.belief_mean[chosen] + 0.15 * rtt;
                self.historical_baseline[chosen] = self.historical_baseline[chosen].min(rtt);
                self.belief_var[chosen] = 8.0;
                self.last_valid_step[chosen] = step;
                self.consecutive_probe_failures[chosen] = 0;
                self.is_failing[chosen] = false;
                false
            }
            None => {
                rtts.push(1200.0);
                self.is_failing[chosen] = true;
                self.consecutive_probe_failures[chosen] += 1;
                let backoff =
                    self.base_backoff * (1 << self.consecutive_probe_failures[chosen].min(5));
                self.next_allowed_probe_step[chosen] = step + backoff;
                true
            }
        };

        // Uncertainty propagation
        for i in 0..4 {
            if i != chosen {
                let age = (step.saturating_sub(self.last_valid_step[i])) as f64;
                self.belief_var[i] = (self.belief_var[i] + 0.2 * age).min(300.0);
            }
        }

        if active_failed {
            let mut candidates: Vec<usize> = (0..4)
                .filter(|&i| i != chosen && !self.is_failing[i])
                .collect();
            if candidates.is_empty() {
                candidates = (0..4).filter(|&i| i != chosen).collect();
            }
            candidates.sort_by(|&a, &b| self.belief_mean[a].total_cmp(&self.belief_mean[b]));

            for &cand in &candidates {
                *probes_spent += 1;
                let res = world.probe(cand, rng);
                if let Some(rtt) = res {
                    *actionable_probes += 1;
                    self.belief_mean[cand] = rtt;
                    self.belief_var[cand] = 8.0;
                    self.last_valid_step[cand] = step;
                    self.consecutive_probe_failures[cand] = 0;
                    self.is_failing[cand] = false;
                    break;
                } else {
                    self.is_failing[cand] = true;
                    self.consecutive_probe_failures[cand] += 1;
                    self.next_allowed_probe_step[cand] = step + self.base_backoff;
                }
            }
        } else {
            // Recovery-Aware Opportunistic Probing
            let active_latency = self.belief_mean[self.current_selected];
            let mut best_gain = 0.0;
            let mut best_target = None;

            for i in 0..4 {
                if i != self.current_selected && step >= self.next_allowed_probe_step[i] {
                    let base = self.historical_baseline[i];
                    let potential_gain = active_latency - base;

                    if potential_gain > self.gain_thresh {
                        let age = step.saturating_sub(self.last_valid_step[i]) as f64;
                        let p_recovered = (1.0 - (-0.03 * age).exp()).max(0.15);
                        let ev = potential_gain * p_recovered;

                        if ev > best_gain {
                            best_gain = ev;
                            best_target = Some(i);
                        }
                    }
                }
            }

            if let Some(target) = best_target {
                *probes_spent += 1;
                let res = world.probe(target, rng);
                if let Some(rtt) = res {
                    self.belief_mean[target] = rtt;
                    self.belief_var[target] = 8.0;
                    self.last_valid_step[target] = step;
                    self.consecutive_probe_failures[target] = 0;
                    self.is_failing[target] = false;
                    if rtt < active_latency {
                        *actionable_probes += 1;
                    }
                } else {
                    self.consecutive_probe_failures[target] += 1;
                    let backoff =
                        self.base_backoff * (1 << self.consecutive_probe_failures[target].min(5));
                    self.next_allowed_probe_step[target] = step + backoff;
                }
            }
        }

        let best = (0..4)
            .filter(|&i| !self.is_failing[i])
            .min_by(|&a, &b| self.belief_mean[a].total_cmp(&self.belief_mean[b]))
            .unwrap_or(3);

        if best != self.current_selected {
            self.current_selected = best;
            *switches += 1;
        }
    }
}

fn evaluate_strategy<S, F>(
    strategy_name: &'static str,
    param_label: String,
    mut init: S,
    mut runner: F,
) -> ParetoMetrics
where
    S: FnMut() -> Box<dyn SchedTrait>,
    F: FnMut(
        &mut dyn SchedTrait,
        usize,
        &StochasticWorld,
        &mut Rng,
        &mut usize,
        &mut usize,
        &mut usize,
        &mut Vec<f64>,
        &mut usize,
    ),
{
    let seeds = 30;
    let mut all_probes = Vec::new();
    let mut all_actionable = Vec::new();
    let mut all_switches = Vec::new();
    let mut all_rtts = Vec::new();
    let mut total_reqs = 0;
    let mut total_succ = 0;

    for s in 0..seeds {
        let seed = 2026110730u64 + (s as u64) * 5437;
        let mut rng = Rng::new(seed);
        let mut world = StochasticWorld::new();
        let mut sched = init();

        let mut probes_spent = 0;
        let mut actionable = 0;
        let mut switches = 0;
        let mut rtts = Vec::with_capacity(1000);
        let mut succ = 0;

        for step in 0..1000 {
            world.advance(step, &mut rng);
            runner(
                sched.as_mut(),
                step,
                &world,
                &mut rng,
                &mut probes_spent,
                &mut actionable,
                &mut switches,
                &mut rtts,
                &mut succ,
            );
        }

        all_probes.push(probes_spent as f64);
        all_actionable.push(actionable as f64);
        all_switches.push(switches as f64);
        total_reqs += 1000;
        total_succ += succ;
        all_rtts.extend(rtts);
    }

    all_rtts.sort_by(|a, b| a.total_cmp(b));
    let n = seeds as f64;
    let actual_probes = all_probes.iter().sum::<f64>() / n;
    let actionable_avg = all_actionable.iter().sum::<f64>() / n;
    let actionable_pct = if actual_probes > 0.0 {
        100.0 * actionable_avg / actual_probes
    } else {
        0.0
    };
    let mean_rtt = all_rtts.iter().sum::<f64>() / all_rtts.len() as f64;
    let p95 = all_rtts[(all_rtts.len() as f64 * 0.95) as usize];
    let p99 = all_rtts[(all_rtts.len() as f64 * 0.99) as usize];
    let success_pct = 100.0 * total_succ as f64 / total_reqs as f64;
    let switches_avg = all_switches.iter().sum::<f64>() / n;

    ParetoMetrics {
        strategy_name,
        param_label,
        actual_probes,
        mean_rtt,
        p95_rtt: p95,
        p99_rtt: p99,
        success_pct,
        actionable_probes_pct: actionable_pct,
        detection_delay_avg: 0.0,
        recovery_delay_avg: 0.0,
        route_switches: switches_avg,
    }
}

fn main() {
    println!(
        "========================================================================================="
    );
    println!("ZERO PROBE SCHEDULING — PARETO BENCHMARK");
    println!(
        "Mapping Pareto Frontier: Cost (Actual Probes / 1,000 steps) vs Latency (Mean User RTT)"
    );
    println!("30 Independent Seeds per Point | Stochastic Markov Outages & Recoveries");
    println!(
        "========================================================================================="
    );

    let mut points: Vec<ParetoMetrics> = Vec::new();

    // 1. Fixed Interval Sweep (Interval = 10, 20, 40, 80, 160)
    for &intv in &[10, 20, 40, 80, 160] {
        let p = evaluate_strategy(
            "Fixed Interval (Mihomo)",
            format!("Interval = {}", intv),
            move || Box::new(FixedScheduler::new(intv)),
            |s, step, w, rng, p, a, sw, r, succ| s.step(step, w, rng, p, a, sw, r, succ),
        );
        points.push(p);
    }

    // 2. Simple Adaptive Heuristic Sweep (Timeout = 15, 30, 60, 120, 250)
    for &timeout in &[15, 30, 60, 120, 250] {
        let p = evaluate_strategy(
            "Simple Adaptive Heuristic",
            format!("Timeout = {}", timeout),
            move || Box::new(SimpleAdaptiveScheduler::new(timeout)),
            |s, step, w, rng, p, a, sw, r, succ| s.step(step, w, rng, p, a, sw, r, succ),
        );
        points.push(p);
    }

    // 3. ZERO Recovery-Aware Sweep (Gain Thresh & Base Backoff combinations)
    let zero_params = [
        (1.0, 2, "Thresh=1ms, Backoff=2 (Ultra)"), // Ultra-aggressive
        (3.0, 4, "Thresh=3ms, Backoff=4 (Fast)"),  // Fast
        (5.0, 5, "Thresh=5ms, Backoff=5"),         // Aggressive
        (10.0, 10, "Thresh=10ms, Backoff=10"),     // Balanced
        (20.0, 15, "Thresh=20ms, Backoff=15"),     // Moderate
        (35.0, 30, "Thresh=35ms, Backoff=30"),     // Conservative
        (50.0, 60, "Thresh=50ms, Backoff=60"),     // Highly frugal
    ];

    for &(thresh, backoff, label) in &zero_params {
        let p = evaluate_strategy(
            "ZERO Recovery-Aware",
            label.to_string(),
            move || Box::new(ZeroScheduler::new(thresh, backoff)),
            |s, step, w, rng, p, a, sw, r, succ| s.step(step, w, rng, p, a, sw, r, succ),
        );
        points.push(p);
    }

    println!("\n=========================================================================================");
    println!("PARETO BENCHMARK RESULTS (SORTED BY STRATEGY & ACTUAL PROBE COST)");
    println!(
        "========================================================================================="
    );
    println!(
        "{:<28} {:<24} {:>10} {:>10} {:>9} {:>9} {:>10} {:>10}",
        "Strategy",
        "Parameters",
        "Probes (X)",
        "Mean (Y)",
        "p95 RTT",
        "p99 RTT",
        "Success%",
        "Actionable%"
    );
    println!("------------------------------------------------------------------------------------------------------------------");

    for pt in &points {
        println!(
            "{:<28} {:<24} {:>9.1} {:>9.1}ms {:>8.1}ms {:>8.1}ms {:>9.2}% {:>9.1}%",
            pt.strategy_name,
            pt.param_label,
            pt.actual_probes,
            pt.mean_rtt,
            pt.p95_rtt,
            pt.p99_rtt,
            pt.success_pct,
            pt.actionable_probes_pct
        );
    }

    // Direct Pareto Comparison at Similar Probe Budgets
    println!("\n=========================================================================================");
    println!("DIRECT PARETO COMPARISON AT SIMILAR PROBE BANDS");
    println!(
        "========================================================================================="
    );

    println!("\n--- BAND 1: ULTRA-LOW PROBE BUDGET (~15 - 35 Probes / 1000 steps) ---");
    let b1_fixed = points
        .iter()
        .find(|p| p.strategy_name.contains("Fixed") && p.param_label.contains("160"))
        .unwrap();
    let b1_heur = points
        .iter()
        .find(|p| p.strategy_name.contains("Adaptive") && p.param_label.contains("120"))
        .unwrap();
    let b1_zero = points
        .iter()
        .find(|p| p.strategy_name.contains("ZERO") && p.param_label.contains("35ms"))
        .unwrap();
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b1_fixed.strategy_name,
        b1_fixed.actual_probes,
        b1_fixed.mean_rtt,
        b1_fixed.p95_rtt,
        b1_fixed.actionable_probes_pct
    );
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b1_heur.strategy_name,
        b1_heur.actual_probes,
        b1_heur.mean_rtt,
        b1_heur.p95_rtt,
        b1_heur.actionable_probes_pct
    );
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b1_zero.strategy_name,
        b1_zero.actual_probes,
        b1_zero.mean_rtt,
        b1_zero.p95_rtt,
        b1_zero.actionable_probes_pct
    );

    println!("\n--- BAND 2: MEDIUM PROBE BUDGET (~40 - 75 Probes / 1000 steps) ---");
    let b2_fixed = points
        .iter()
        .find(|p| p.strategy_name.contains("Fixed") && p.param_label.contains("80"))
        .unwrap();
    let b2_heur = points
        .iter()
        .find(|p| p.strategy_name.contains("Adaptive") && p.param_label.contains("60"))
        .unwrap();
    let b2_zero = points
        .iter()
        .find(|p| p.strategy_name.contains("ZERO") && p.param_label.contains("10ms"))
        .unwrap();
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b2_fixed.strategy_name,
        b2_fixed.actual_probes,
        b2_fixed.mean_rtt,
        b2_fixed.p95_rtt,
        b2_fixed.actionable_probes_pct
    );
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b2_heur.strategy_name,
        b2_heur.actual_probes,
        b2_heur.mean_rtt,
        b2_heur.p95_rtt,
        b2_heur.actionable_probes_pct
    );
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b2_zero.strategy_name,
        b2_zero.actual_probes,
        b2_zero.mean_rtt,
        b2_zero.p95_rtt,
        b2_zero.actionable_probes_pct
    );

    println!("\n--- BAND 3: HIGH PROBE BUDGET (~100 - 200 Probes / 1000 steps) ---");
    let b3_fixed = points
        .iter()
        .find(|p| p.strategy_name.contains("Fixed") && p.param_label.contains("20"))
        .unwrap();
    let b3_heur = points
        .iter()
        .find(|p| p.strategy_name.contains("Adaptive") && p.param_label.contains("15"))
        .unwrap();
    let b3_zero = points
        .iter()
        .find(|p| p.strategy_name.contains("ZERO") && p.param_label.contains("5ms"))
        .unwrap();
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b3_fixed.strategy_name,
        b3_fixed.actual_probes,
        b3_fixed.mean_rtt,
        b3_fixed.p95_rtt,
        b3_fixed.actionable_probes_pct
    );
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b3_heur.strategy_name,
        b3_heur.actual_probes,
        b3_heur.mean_rtt,
        b3_heur.p95_rtt,
        b3_heur.actionable_probes_pct
    );
    println!(
        "{:<28} -> Probes: {:>5.1} | Mean RTT: {:>6.1}ms | p95: {:>6.1}ms | Actionable: {:>5.1}%",
        b3_zero.strategy_name,
        b3_zero.actual_probes,
        b3_zero.mean_rtt,
        b3_zero.p95_rtt,
        b3_zero.actionable_probes_pct
    );

    println!(
        "========================================================================================="
    );
}
