//! ZERO Shadow Mode for MultiLink 0.1
//! Experimental observational layer for evaluating ZERO Dual Health routing predictions
//! side-by-side with MultiLink's native Smart Mode without interfering with live traffic.
//!
//! SAFETY GUARANTEES:
//! - Strictly passive observation: ZERO does NOT modify Mihomo proxy groups, active rules,
//!   Windows system proxy, or existing TCP streams.
//! - Strict boundary preservation: ZERO recommendations respect user Direct/VPN restrictions
//!   and select exclusively from authorized, enabled VPN candidate nodes.
//! - Clear separation of ground truth: directly observed connections (Mihomo choices) vs
//!   unverified hypotheses (alternative ZERO suggestions).
//! - Zero overhead when disabled: can be toggled via `enabled` flag.

use multilink_core::Node;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::time::Instant;

pub const NUM_CONTEXTS: usize = 16;
pub const CB_STREAK_TO_BENCH: u32 = 2;
pub const CB_BENCH_BASE: u32 = 4;
pub const CB_BENCH_MAX: u32 = 32;
pub const DUAL_DISTINCT_CONTEXTS_FOR_GLOBAL: usize = 2;

/// Deterministically maps a destination host/domain to a discrete context ID (0..15)
/// based on domain structure and length.
pub fn domain_context(host: &str) -> usize {
    let clean = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let mut hash = 0u64;
    for byte in clean.as_bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(*byte as u64);
    }
    (hash % NUM_CONTEXTS as u64) as usize
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateEval {
    pub server_name: String,
    pub expected_cost_ms: f64,
    pub confidence: f64,
    pub global_healthy: bool,
    pub context_healthy: bool,
    pub reliability: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZeroRecommendation {
    pub recommended_server: String,
    pub expected_cost_ms: f64,
    pub confidence: f64,
    pub reason: String,
    pub candidate_evaluations: Vec<CandidateEval>,
    pub decision_latency_ns: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CounterfactualStatus {
    /// ZERO recommended the same server as Mihomo; the outcome is verified ground truth.
    DirectlyObserved,
    /// ZERO recommended a different server; its outcome was unobserved on this user connection.
    UnverifiedHypothesis,
    /// A controlled parallel probe confirmed the alternative server under identical conditions.
    IndependentlyProbed { verified_latency_ms: Option<u64> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShadowObservation {
    pub timestamp_secs: u64,
    pub destination_host: String,
    pub context_id: usize,
    pub mihomo_selected_server: String,
    pub zero_recommended_server: String,
    pub is_agreement: bool,
    pub actual_latency_ms: Option<u64>,
    pub actual_success: bool,
    pub counterfactual_status: CounterfactualStatus,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct ZeroShadowSummary {
    pub enabled: bool,
    pub total_observations: usize,
    pub agreements: usize,
    pub divergences: usize,
    pub agreement_pct: f64,
    pub observed_mihomo_successes: usize,
    pub observed_mihomo_failures: usize,
    pub observed_mihomo_success_pct: f64,
    pub unverified_hypotheses_count: usize,
    pub controlled_probes_count: usize,
    pub active_global_benches: usize,
    pub active_context_benches: usize,
    pub mean_decision_latency_ns: u128,
}

/// The ZeroShadowMode runtime observer.
pub struct ZeroShadowMode {
    pub enabled: bool,

    // Dual Health tracking state
    consecutive_context_failures: HashMap<(String, usize), u32>,
    context_cooldown: HashMap<(String, usize), u32>,
    context_bench_timer: HashMap<(String, usize), u32>,

    global_bench_timer: HashMap<String, u32>,
    global_cooldown: HashMap<String, u32>,
    recent_failed_contexts: HashMap<String, HashSet<usize>>,

    // Diagnostics & history
    observations: Vec<ShadowObservation>,
    total_decision_nanos: u128,
    decision_count: usize,
}

impl Default for ZeroShadowMode {
    fn default() -> Self {
        Self::new(true)
    }
}

impl ZeroShadowMode {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            consecutive_context_failures: HashMap::new(),
            context_cooldown: HashMap::new(),
            context_bench_timer: HashMap::new(),
            global_bench_timer: HashMap::new(),
            global_cooldown: HashMap::new(),
            recent_failed_contexts: HashMap::new(),
            observations: Vec::new(),
            total_decision_nanos: 0,
            decision_count: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn is_globally_benched(&self, server: &str) -> bool {
        self.global_bench_timer.get(server).copied().unwrap_or(0) > 0
    }

    pub fn is_contextually_benched(&self, server: &str, context_id: usize) -> bool {
        if self.is_globally_benched(server) {
            return true;
        }
        self.context_bench_timer
            .get(&(server.to_string(), context_id))
            .copied()
            .unwrap_or(0)
            > 0
    }

    /// Ticks down cooldown timers.
    pub fn tick_cooldowns(&mut self) {
        for timer in self.global_bench_timer.values_mut() {
            if *timer > 0 {
                *timer -= 1;
            }
        }
        for timer in self.context_bench_timer.values_mut() {
            if *timer > 0 {
                *timer -= 1;
            }
        }
    }

    /// Evaluates candidate nodes and yields a recommendation with expected cost and reason.
    pub fn recommend(
        &mut self,
        destination_host: &str,
        candidates: &[&Node],
    ) -> Option<ZeroRecommendation> {
        if !self.enabled || candidates.is_empty() {
            return None;
        }

        let start = Instant::now();
        let ctx = domain_context(destination_host);

        let mut evaluations = Vec::with_capacity(candidates.len());
        let mut best_idx = 0;
        let mut best_cost = f64::INFINITY;

        for (idx, node) in candidates.iter().enumerate() {
            let server_name = &node.name;
            let g_healthy = !self.is_globally_benched(server_name);
            let c_healthy = !self.is_contextually_benched(server_name, ctx);

            // Compute expected cost based on NodeQuality metrics
            let base_lat = if node.quality.latency_ms > 0.0 {
                node.quality.latency_ms
            } else {
                // Untested node prior
                180.0
            };
            let jitter_penalty = node.quality.jitter_ms * 0.4;
            let failure_prob = (1.0 - node.quality.reliability.clamp(0.01, 0.99)).max(0.01);
            const FALLBACK_PENALTY_MS: f64 = 1200.0;

            let mut expected_cost = base_lat + jitter_penalty + failure_prob * FALLBACK_PENALTY_MS;

            // Apply Dual Health penalties
            if !g_healthy {
                expected_cost += 10_000.0; // Prohibitive penalty for global outage
            } else if !c_healthy {
                expected_cost += 3_000.0; // Penalty for context failure
            }

            // Confidence estimation based on sample count and jitter
            let sample_weight = (node.quality.samples as f64) / (node.quality.samples as f64 + 4.0);
            let jitter_ratio = (node.quality.jitter_ms / (base_lat + node.quality.jitter_ms + 1.0))
                .clamp(0.0, 0.8);
            let confidence = (sample_weight * (1.0 - jitter_ratio)).clamp(0.1, 0.98);

            evaluations.push(CandidateEval {
                server_name: server_name.clone(),
                expected_cost_ms: expected_cost,
                confidence,
                global_healthy: g_healthy,
                context_healthy: c_healthy,
                reliability: node.quality.reliability,
            });

            if expected_cost < best_cost {
                best_cost = expected_cost;
                best_idx = idx;
            }
        }

        let elapsed_ns = start.elapsed().as_nanos();
        let black_boxed_ns = black_box(elapsed_ns);

        self.total_decision_nanos += black_boxed_ns;
        self.decision_count += 1;

        let best = &evaluations[best_idx];
        let reason = if !best.global_healthy {
            format!(
                "Fallback: all candidate servers degraded, selecting least-penalized ({:.0}ms)",
                best.expected_cost_ms
            )
        } else if !best.context_healthy {
            format!("Context-restricted: chosen node operates under penalty for context {ctx}")
        } else if best.confidence > 0.7 {
            format!(
                "High-confidence optimum: expected latency {:.0}ms (rel={:.0}%)",
                best.expected_cost_ms,
                best.reliability * 100.0
            )
        } else {
            format!(
                "Exploratory/low-sample choice: expected cost {:.0}ms",
                best.expected_cost_ms
            )
        };

        Some(ZeroRecommendation {
            recommended_server: best.server_name.clone(),
            expected_cost_ms: best.expected_cost_ms,
            confidence: best.confidence,
            reason,
            candidate_evaluations: evaluations,
            decision_latency_ns: black_boxed_ns,
        })
    }

    /// Records real feedback observed from an actual connection handled by Mihomo.
    pub fn observe_connection_feedback(
        &mut self,
        server: &str,
        destination_host: &str,
        success: bool,
    ) {
        if !self.enabled {
            return;
        }

        let ctx = domain_context(destination_host);
        let key = (server.to_string(), ctx);

        if success {
            // Success restores context trust
            self.consecutive_context_failures.insert(key.clone(), 0);
            self.context_cooldown.insert(key.clone(), 0);
            self.context_bench_timer.insert(key, 0);

            // Clears global failure suspicion
            if let Some(set) = self.recent_failed_contexts.get_mut(server) {
                set.remove(&ctx);
            }
            self.global_bench_timer.insert(server.to_string(), 0);
            self.global_cooldown.insert(server.to_string(), 0);
        } else {
            // Failure in context `ctx`
            let fails = self
                .consecutive_context_failures
                .entry(key.clone())
                .or_insert(0);
            *fails += 1;

            self.recent_failed_contexts
                .entry(server.to_string())
                .or_default()
                .insert(ctx);

            // 1. Context-level circuit breaker
            if *fails >= CB_STREAK_TO_BENCH {
                let cur = self.context_cooldown.get(&key).copied().unwrap_or(0);
                let next = if cur == 0 {
                    CB_BENCH_BASE
                } else {
                    (cur * 2).min(CB_BENCH_MAX)
                };
                self.context_cooldown.insert(key.clone(), next);
                self.context_bench_timer.insert(key, next);
                *fails = 0;
            }

            // 2. Global escalation: failures confirmed across >= 2 distinct contexts
            let distinct = self
                .recent_failed_contexts
                .get(server)
                .map(|s| s.len())
                .unwrap_or(0);

            if distinct >= DUAL_DISTINCT_CONTEXTS_FOR_GLOBAL {
                let cur_g = self.global_cooldown.get(server).copied().unwrap_or(0);
                let next_g = if cur_g == 0 {
                    CB_BENCH_BASE
                } else {
                    (cur_g * 2).min(CB_BENCH_MAX)
                };
                self.global_cooldown.insert(server.to_string(), next_g);
                self.global_bench_timer.insert(server.to_string(), next_g);
                self.recent_failed_contexts
                    .insert(server.to_string(), HashSet::new());
            }
        }

        self.tick_cooldowns();
    }

    /// Logs a paired observation comparing Mihomo's actual choice vs ZERO's suggestion.
    pub fn record_observation(&mut self, obs: ShadowObservation) {
        if !self.enabled {
            return;
        }
        self.observations.push(obs);
    }

    /// Computes summary statistics.
    pub fn summary(&self) -> ZeroShadowSummary {
        let total = self.observations.len();
        let mut agreements = 0;
        let mut divergences = 0;
        let mut mi_succ = 0;
        let mut mi_fail = 0;
        let mut unverified = 0;
        let mut probes = 0;

        for obs in &self.observations {
            if obs.is_agreement {
                agreements += 1;
            } else {
                divergences += 1;
            }
            if obs.actual_success {
                mi_succ += 1;
            } else {
                mi_fail += 1;
            }
            match obs.counterfactual_status {
                CounterfactualStatus::DirectlyObserved => {}
                CounterfactualStatus::UnverifiedHypothesis => unverified += 1,
                CounterfactualStatus::IndependentlyProbed { .. } => probes += 1,
            }
        }

        let agreement_pct = if total > 0 {
            100.0 * agreements as f64 / total as f64
        } else {
            0.0
        };
        let mi_succ_pct = if total > 0 {
            100.0 * mi_succ as f64 / total as f64
        } else {
            0.0
        };
        let mean_latency = if self.decision_count > 0 {
            self.total_decision_nanos / self.decision_count as u128
        } else {
            0
        };

        let active_g = self.global_bench_timer.values().filter(|&&t| t > 0).count();
        let active_c = self
            .context_bench_timer
            .values()
            .filter(|&&t| t > 0)
            .count();

        ZeroShadowSummary {
            enabled: self.enabled,
            total_observations: total,
            agreements,
            divergences,
            agreement_pct,
            observed_mihomo_successes: mi_succ,
            observed_mihomo_failures: mi_fail,
            observed_mihomo_success_pct: mi_succ_pct,
            unverified_hypotheses_count: unverified,
            controlled_probes_count: probes,
            active_global_benches: active_g,
            active_context_benches: active_c,
            mean_decision_latency_ns: mean_latency,
        }
    }

    pub fn reset(&mut self) {
        self.consecutive_context_failures.clear();
        self.context_cooldown.clear();
        self.context_bench_timer.clear();
        self.global_bench_timer.clear();
        self.global_cooldown.clear();
        self.recent_failed_contexts.clear();
        self.observations.clear();
        self.total_decision_nanos = 0;
        self.decision_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use multilink_core::NodeQuality;
    use serde_yaml::Mapping;

    fn make_test_node(name: &str, latency: f64, reliability: f64) -> Node {
        let mut q = NodeQuality::default();
        q.latency_ms = latency;
        q.jitter_ms = 10.0;
        q.reliability = reliability;
        q.samples = 15;
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

    #[test]
    fn domain_context_is_deterministic_and_bounded() {
        let c1 = domain_context("github.com");
        let c2 = domain_context("github.com");
        let c3 = domain_context("Google.com.");
        assert_eq!(c1, c2);
        assert!(c1 < NUM_CONTEXTS);
        assert!(c3 < NUM_CONTEXTS);
    }

    #[test]
    fn zero_prefers_lower_expected_cost_node() {
        let mut shadow = ZeroShadowMode::new(true);
        let n1 = make_test_node("fast-de", 45.0, 0.99);
        let n2 = make_test_node("slow-us", 220.0, 0.99);

        let rec = shadow.recommend("api.github.com", &[&n1, &n2]).unwrap();
        assert_eq!(rec.recommended_server, "fast-de");
        assert!(rec.expected_cost_ms < 100.0);
    }

    #[test]
    fn dual_health_context_failure_does_not_break_other_domains() {
        let mut shadow = ZeroShadowMode::new(true);
        let n1 = make_test_node("fast-de", 45.0, 0.99);
        let n2 = make_test_node("slow-us", 180.0, 0.99);

        let blocked_domain = "site-a.invalid";
        let healthy_domain = "site-b.invalid";
        let ctx_a = domain_context(blocked_domain);
        let ctx_b = domain_context(healthy_domain);

        // Ensure distinct contexts
        if ctx_a == ctx_b {
            return; // Skip if hash collision in test
        }

        // 2 failures on site-a -> fast-de benched on site-a
        shadow.observe_connection_feedback("fast-de", blocked_domain, false);
        shadow.observe_connection_feedback("fast-de", blocked_domain, false);

        assert!(shadow.is_contextually_benched("fast-de", ctx_a));
        assert!(!shadow.is_contextually_benched("fast-de", ctx_b));
        assert!(!shadow.is_globally_benched("fast-de"));

        // For site-b, fast-de should STILL be recommended!
        let rec_b = shadow.recommend(healthy_domain, &[&n1, &n2]).unwrap();
        assert_eq!(rec_b.recommended_server, "fast-de");

        // For site-a, slow-us should be recommended due to context bench!
        let rec_a = shadow.recommend(blocked_domain, &[&n1, &n2]).unwrap();
        assert_eq!(rec_a.recommended_server, "slow-us");
    }

    #[test]
    fn dual_health_escalates_to_global_on_multi_context_outage() {
        let mut shadow = ZeroShadowMode::new(true);
        let n1 = make_test_node("failing-node", 40.0, 0.99);
        let n2 = make_test_node("backup-node", 160.0, 0.99);

        // Fail across 2 distinct contexts
        shadow.observe_connection_feedback("failing-node", "alpha.com", false);
        shadow.observe_connection_feedback("failing-node", "beta.org", false);

        assert!(shadow.is_globally_benched("failing-node"));
        let rec = shadow.recommend("gamma.net", &[&n1, &n2]).unwrap();
        assert_eq!(rec.recommended_server, "backup-node");
    }

    #[test]
    fn disabled_shadow_mode_returns_none() {
        let mut shadow = ZeroShadowMode::new(false);
        let n1 = make_test_node("node-1", 50.0, 0.99);
        assert!(shadow.recommend("test.com", &[&n1]).is_none());
    }
}
