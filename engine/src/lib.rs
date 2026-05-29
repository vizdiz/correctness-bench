// engine — built supervised
//
// Worker + coordinator: COO-correct scheduler, HDR histograms (corrected +
// uncorrected), gRPC streaming per contracts/bench.proto, fleet split/sync/merge.
// This is the hot path and the riskiest code in the project. It is implemented
// with a human in the loop and must pass gates/gate1_wrk2_agreement.md before
// anything downstream matters. Do not implement unsupervised.
