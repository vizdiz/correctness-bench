//! Property tests on the COO scheduler. Invariants from the design doc:
//!   - across any sequence of advances, intended-send times are non-decreasing
//!   - usec_to_next_send never returns a negative value (it's u64)
//!   - when behind, the wait at catch-up rate is strictly less than the wait
//!     at the original rate would be (catch-up = 2× original)

use engine::sched::ConnSched;
use proptest::prelude::*;

proptest! {
    // Intended-send times are strictly non-decreasing in `complete`.
    #[test]
    fn intended_send_times_monotonic(
        thread_start_us in 0u64..1_000_000,
        rps in 1.0f64..100_000.0,
        n in 1usize..1_000,
    ) {
        let throughput_us = rps / 1_000_000.0;
        let mut s = ConnSched::new(thread_start_us, throughput_us);
        let mut prev = s.intended_send_time();
        for _ in 0..n {
            s.advance();
            let now = s.intended_send_time();
            prop_assert!(now >= prev);
            prev = now;
        }
    }

    // Behind by any positive amount -> wait is finite and <= the original gap.
    #[test]
    fn catch_up_never_overshoots(
        rps in 1.0f64..100_000.0,
        behind_us in 1u64..10_000_000,
        complete in 1u64..1_000,
    ) {
        let throughput_us = rps / 1_000_000.0;
        let mut s = ConnSched::new(0, throughput_us);
        s.complete = complete;
        let intended = s.intended_send_time();
        let now = intended + behind_us;
        let wait = s.usec_to_next_send(now);
        // We're behind -> wait should be 0 (fire now) OR a small catch-up gap.
        // The catch-up gap should never exceed the original interval (1/rps).
        let original_interval_us = (1_000_000.0 / rps) as u64;
        prop_assert!(wait <= original_interval_us.saturating_add(1));
    }
}
