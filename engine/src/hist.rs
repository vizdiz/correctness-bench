//! HDR histogram wrapper for corrected + uncorrected latency.
//!
//! Range: 1 µs to 60 s, 3 significant digits — wrk2 uses 24 hours; 60 s is
//! enough for any HTTP response that wouldn't trigger a timeout anyway, and
//! keeps memory modest. Histograms are mergeable and V2-serializable for
//! later transport to the coordinator (gate #3) and storage.

use hdrhistogram::Histogram;

const MIN_US: u64 = 1;
const MAX_US: u64 = 60_000_000;
const SIG_FIGS: u8 = 3;

/// One worker's per-stream pair: latency from the *intended* send time
/// (corrected) plus latency from the actual send time (uncorrected). Their
/// delta proves the COO correction is doing work — gate #1 checks both.
#[derive(Debug, Clone)]
pub struct Histos {
    pub corrected: Histogram<u64>,
    pub uncorrected: Histogram<u64>,
}

impl Histos {
    pub fn new() -> Self {
        Self {
            corrected: Histogram::new_with_bounds(MIN_US, MAX_US, SIG_FIGS).unwrap(),
            uncorrected: Histogram::new_with_bounds(MIN_US, MAX_US, SIG_FIGS).unwrap(),
        }
    }
    /// Record a single response. Both latencies are saturated to the histogram
    /// range to avoid lossy out-of-range drops — being out of range is itself
    /// data we want surfaced as the tail percentile.
    pub fn record(&mut self, corrected_us: u64, uncorrected_us: u64) {
        let c = corrected_us.clamp(MIN_US, MAX_US);
        let u = uncorrected_us.clamp(MIN_US, MAX_US);
        let _ = self.corrected.record(c);
        let _ = self.uncorrected.record(u);
    }
    pub fn merge(&mut self, other: &Histos) {
        self.corrected.add(&other.corrected).unwrap();
        self.uncorrected.add(&other.uncorrected).unwrap();
    }
}

impl Default for Histos {
    fn default() -> Self {
        Self::new()
    }
}

/// Pretty-printable summary for the CLI.
#[derive(Debug, Clone)]
pub struct Summary {
    pub count: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub max_us: u64,
}

impl Summary {
    pub fn from(h: &Histogram<u64>) -> Self {
        Self {
            count: h.len(),
            p50_us: h.value_at_quantile(0.50),
            p95_us: h.value_at_quantile(0.95),
            p99_us: h.value_at_quantile(0.99),
            p999_us: h.value_at_quantile(0.999),
            max_us: h.max(),
        }
    }
}
