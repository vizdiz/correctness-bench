//! Coordinated-omission-correct scheduler — a faithful port of wrk2's
//! `usec_to_next_send` (giltene/wrk2 `src/wrk.c`).
//!
//! The invariant gate #1 verifies: **latency is measured from the *intended*
//! send time, not the actual send time.** When the client falls behind, we
//! catch up at `throughput * 2.0` (matched to wrk2 deliberately so the gate
//! holds), but the latency for each request is always relative to its original
//! intended slot on the schedule. The intended slot for request `n` on a
//! connection is `thread_start + n / throughput_us` — independent of catch-up.
//!
//! This module is pure (no async, no clock — `now_us` is an argument), so it
//! can be exhaustively unit-tested and property-tested.

/// Rate-shape of the schedule. Constant is the gate-#1 mode (wrk2-equivalent);
/// Ramp linearly accelerates 0 → peak over `duration_us`, the intended-send
/// timing inverts the cumulative count integral `N(t) = R·t²/(2·D)` →
/// `t(N) = sqrt(2·D·N/R)`.
#[derive(Debug, Clone, Copy)]
pub enum RateSchedule {
    Constant { throughput_us: f64 },
    Ramp { duration_us: u64, peak_throughput_us: f64 },
}

/// Per-connection scheduling state. One per connection task.
#[derive(Debug, Clone)]
pub struct ConnSched {
    /// Wall time (microseconds since some epoch) when this connection started.
    /// Shared across all connections in a worker (per-thread in wrk2 parlance).
    pub thread_start_us: u64,
    /// Number of completed responses on this connection — the index that
    /// defines each request's intended send slot.
    pub complete: u64,
    /// Rate-shape of the schedule (Constant or Ramp).
    pub schedule: RateSchedule,
    /// Catch-up rate — wrk2 hardcodes 2× throughput. Stored explicitly so a
    /// future tuner can probe other values; gate #1 needs 2×. Only meaningful
    /// for Constant — Ramp's instantaneous rate makes the wrk2 catch-up trick
    /// awkward; behind-on-Ramp simply fires immediately.
    pub catch_up_us: f64,
    /// True when we're on or ahead of the original schedule. Transitions
    /// false → true on each "we re-caught up" cycle, anchoring catch-up windows.
    pub caught_up: bool,
    /// When we last fell behind. Defines the catch-up window's origin.
    pub catch_up_start_time: u64,
    /// `complete` at the moment we last fell behind. Subtracting this from the
    /// current `complete` gives the catch-up window's elapsed request count.
    pub complete_at_catch_up_start: u64,
}

impl ConnSched {
    /// Constant-rate schedule (the gate-#1 mode). Catch-up rate is wrk2's 2×
    /// (do not change without invalidating gate #1 agreement).
    pub fn new(thread_start_us: u64, throughput_us: f64) -> Self {
        Self {
            thread_start_us,
            complete: 0,
            schedule: RateSchedule::Constant { throughput_us },
            catch_up_us: throughput_us * 2.0,
            caught_up: true,
            catch_up_start_time: 0,
            complete_at_catch_up_start: 0,
        }
    }

    /// Linear ramp 0 → peak_throughput_us over duration_us.
    pub fn ramp(thread_start_us: u64, duration_us: u64, peak_throughput_us: f64) -> Self {
        Self {
            thread_start_us,
            complete: 0,
            schedule: RateSchedule::Ramp { duration_us, peak_throughput_us },
            catch_up_us: 0.0,
            caught_up: true,
            catch_up_start_time: 0,
            complete_at_catch_up_start: 0,
        }
    }

    /// The intended send time for the request currently being scheduled (the
    /// `complete`-th one, 0-indexed). This is what corrected latency is measured
    /// against, and it is computed from the ORIGINAL schedule — catch-up never
    /// changes how we measure, only how we pace.
    pub fn intended_send_time(&self) -> u64 {
        match self.schedule {
            RateSchedule::Constant { throughput_us } => {
                self.thread_start_us + (self.complete as f64 / throughput_us) as u64
            }
            RateSchedule::Ramp { duration_us, peak_throughput_us } => {
                let n = self.complete as f64;
                let t_us =
                    (2.0 * duration_us as f64 * n / peak_throughput_us).sqrt() as u64;
                self.thread_start_us + t_us
            }
        }
    }

    /// Microseconds the caller should sleep before firing the next request. 0
    /// means "fire now." For Constant, ports wrk2's `usec_to_next_send`
    /// catch-up-at-2× behavior. For Ramp, fires immediately when behind (no
    /// catch-up bump — the instantaneous rate already changes with time).
    pub fn usec_to_next_send(&mut self, now_us: u64) -> u64 {
        let next_start_orig = self.intended_send_time();

        if next_start_orig > now_us {
            // We are on pace. Indicate we're caught up so the next "fall behind"
            // anchors a fresh catch-up window.
            self.caught_up = true;
            return next_start_orig - now_us;
        }

        // Behind.
        if matches!(self.schedule, RateSchedule::Ramp { .. }) {
            // Ramp has no catch-up bump — fire immediately.
            self.caught_up = false;
            return 0;
        }

        // Constant + behind: wrk2 catch-up at 2×.
        if self.caught_up {
            self.caught_up = false;
            self.catch_up_start_time = now_us;
            self.complete_at_catch_up_start = self.complete;
        }

        let since = self.complete - self.complete_at_catch_up_start;
        let catch_up_next = self.catch_up_start_time + (since as f64 / self.catch_up_us) as u64;
        if catch_up_next > now_us {
            catch_up_next - now_us
        } else {
            0
        }
    }

    /// Bump completion count after the response arrives.
    pub fn advance(&mut self) {
        self.complete += 1;
    }

    /// Shift the schedule origin forward by `delta_us`. Used when the worker
    /// deliberately pauses (e.g. honoring a 429 `Retry-After`): pushing
    /// `thread_start_us` forward moves every future intended-send time forward
    /// by the same amount, so the deferred requests are NOT charged the
    /// voluntary wait as (target) latency. This keeps the COO correction honest
    /// — the backoff behaves like a schedule pause, not like target slowness —
    /// while naturally reducing effective RPS by the skipped slots.
    ///
    /// The catch-up window is re-armed (`caught_up = true`) so we resume at the
    /// normal rate after the pause instead of blasting a catch-up burst that
    /// would immediately re-trip the limiter.
    pub fn defer_origin(&mut self, delta_us: u64) {
        self.thread_start_us = self.thread_start_us.saturating_add(delta_us);
        self.caught_up = true;
    }
}

// ============================================================
// Tests — pure logic, no clocks.
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// At steady state on a 1000 RPS schedule with 1 connection, the gap
    /// between consecutive intended-send times is 1000 µs.
    #[test]
    fn intended_send_times_step_by_the_interval() {
        let mut s = ConnSched::new(0, 1000.0 / 1_000_000.0); // 1000 rps -> 0.001 req/us
        let t0 = s.intended_send_time();
        s.advance();
        let t1 = s.intended_send_time();
        s.advance();
        let t2 = s.intended_send_time();
        assert_eq!(t1 - t0, 1000);
        assert_eq!(t2 - t1, 1000);
    }

    /// When the client is exactly on schedule, the next-send wait is zero or
    /// positive and never overshoots the original slot.
    #[test]
    fn on_schedule_returns_remaining_wait() {
        let mut s = ConnSched::new(0, 0.001); // 1000 rps
        assert_eq!(s.usec_to_next_send(0), 0); // first request — fire now
        s.advance();
        // Next intended at t=1000us. At t=600us we have 400us left.
        assert_eq!(s.usec_to_next_send(600), 400);
    }

    /// If we're behind, wait drops to 0 and catch-up engages.
    #[test]
    fn behind_engages_catch_up() {
        let mut s = ConnSched::new(0, 0.001); // 1000 rps, 1000us interval
        // First fire was at t=0. Pretend we got back to scheduling 3000us late.
        s.advance(); // complete=1, intended t=1000us
        let wait = s.usec_to_next_send(4000); // intended 1000us, now 4000us -> behind
        assert_eq!(wait, 0); // fire immediately
        assert!(!s.caught_up);
        // After firing, complete=2. Catch-up rate is 2x -> 500us spacing.
        s.advance(); // complete=2
        let wait = s.usec_to_next_send(4000); // 1 since catch-up start, catch_up_us=0.002
                                              // catch_up_next = 4000 + (1 / 0.002) = 4500
        assert_eq!(wait, 500);
    }

    /// Catching back up flips the `caught_up` flag and re-arms the window.
    #[test]
    fn catch_up_flag_resets_when_back_on_pace() {
        let mut s = ConnSched::new(0, 0.001);
        s.advance();
        s.usec_to_next_send(5000); // way behind -> not caught up
        assert!(!s.caught_up);
        // Now pretend many requests later we're ahead of schedule.
        s.complete = 50;
        s.usec_to_next_send(10_000); // intended ~50_000, now 10_000 -> ahead
        assert!(s.caught_up);
    }

    /// Property: `complete / throughput_us` is monotonically non-decreasing in
    /// `complete`, so successive intended-send times never go backwards.
    #[test]
    fn intended_send_times_are_monotonic() {
        let mut s = ConnSched::new(123456, 0.0017);
        let mut prev = s.intended_send_time();
        for _ in 0..10_000 {
            s.advance();
            let now = s.intended_send_time();
            assert!(now >= prev, "intended-send-time went backwards");
            prev = now;
        }
    }

    /// Ramp's first request fires immediately (n=0 → t=0); subsequent intended
    /// times follow the sqrt curve. After N(D)=R·D/2 requests, intended time
    /// equals D (the run end). Verified at 1000 RPS peak over 1 s.
    #[test]
    fn ramp_intended_times_match_inverse_integral() {
        // Peak 1000 RPS for 1s → 500 expected requests; intended time at the
        // 500th matches D (1 000 000 µs) to within rounding.
        let mut s = ConnSched::ramp(0, 1_000_000, 0.001);
        assert_eq!(s.intended_send_time(), 0, "n=0 fires at the start");
        let mut prev = 0u64;
        for _ in 0..500 {
            s.advance();
            let t = s.intended_send_time();
            assert!(t >= prev, "ramp intended time regressed: {} < {}", t, prev);
            prev = t;
        }
        // At complete=500, t = sqrt(2·1e6·500/0.001) ≈ 1e6.
        let t_end = s.intended_send_time();
        assert!(t_end >= 990_000 && t_end <= 1_010_000, "ramp end time = {}", t_end);
    }

    /// Ramp doesn't engage wrk2-style catch-up. Behind on ramp → fire immediately.
    #[test]
    fn ramp_fires_immediately_when_behind() {
        let mut s = ConnSched::ramp(0, 1_000_000, 0.001);
        s.advance(); // n=1, intended around sqrt(2e9) ≈ 44721 µs
        let wait = s.usec_to_next_send(1_000_000);
        assert_eq!(wait, 0); // far behind, fire now
        assert!(!s.caught_up);
    }
}
