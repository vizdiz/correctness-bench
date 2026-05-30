//! engine — single-worker, COO-correct load generator (gate #1 MVP).
//!
//! The coordinator + gRPC + fleet are not in this crate yet; the build kit
//! orders them after gate #1 is green. See `gates/gate1_wrk2_agreement.md`.

pub mod hist;
pub mod sched;
pub mod worker;

pub use hist::{Histos, Summary};
pub use sched::ConnSched;
pub use worker::{run, run_with_ticks, RunReport, RunSpec, Tick};
