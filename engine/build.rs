// Generate tonic + prost code for the frozen worker↔coordinator gRPC contract.
// The proto lives outside this crate (it's a TOP-LEVEL repo asset shared by
// engine and the eventual control-plane gRPC client), so we point at the
// repo-root contracts dir explicitly.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../contracts/bench.proto";
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&[proto], &["../contracts"])?;
    println!("cargo:rerun-if-changed={}", proto);
    Ok(())
}
