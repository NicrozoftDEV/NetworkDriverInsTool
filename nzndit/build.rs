//! Embed a Windows manifest: asInvoker (scanning runs unelevated; install checks
//! admin at runtime), Common Controls v6 (themed native controls), and System
//! DPI awareness.
//!
//! DPI awareness must be `System`, *not* Per-Monitor: nwg's `high-dpi` feature
//! derives its scale factor from the legacy GDI path
//! (`GetDeviceCaps(screen, LOGPIXELSX)`), which returns the real system DPI only
//! for a system-aware process. A per-monitor-aware process always reads 96 there
//! (the OS expects it to call `GetDpiForWindow`, which nwg doesn't), so the scale
//! factor stays 1.0, the hard-coded control coordinates never scale, and the
//! window renders tiny on a high-DPI display. nwg 1.0 also ignores
//! `WM_DPICHANGED`, so per-monitor awareness would buy nothing anyway. Under
//! System awareness the UI is crisp at the primary monitor's scale; Windows
//! bitmap-stretches it on differently-scaled monitors.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        use embed_manifest::manifest::{DpiAwareness, ExecutionLevel};
        use embed_manifest::{embed_manifest, new_manifest};
        embed_manifest(
            new_manifest("Nicrozoft.NZNDIT")
                .requested_execution_level(ExecutionLevel::AsInvoker)
                .dpi_awareness(DpiAwareness::System),
        )
        .expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
