//! Embed a Windows application manifest declaring `asInvoker`.
//!
//! Without an explicit execution level, Windows' UAC *installer-detection*
//! heuristic auto-requires elevation for any executable whose name contains
//! "install"/"setup"/"update" — so the binary would refuse to launch
//! unelevated. Declaring a level disables that heuristic; `detect`/`match` then
//! run as a normal user, and `install` checks for admin rights at runtime.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        use embed_manifest::{embed_manifest, manifest::ExecutionLevel, new_manifest};
        embed_manifest(
            new_manifest("NicInstaller").requested_execution_level(ExecutionLevel::AsInvoker),
        )
        .expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
