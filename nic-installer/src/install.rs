//! Install a driver package via the Windows driver-install API `DiInstallDriverW`
//! (newdev.dll) — the same call `pnputil /add-driver … /install` makes: it adds
//! the INF's package to the driver store and installs it to matching present
//! devices. No external process, no console.

use anyhow::{bail, Result};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{DiInstallDriverW, DIINSTALLDRIVER_FLAGS};
use windows::Win32::Foundation::{BOOL, HWND};

pub fn find_infs(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p
                    .extension()
                    .map(|x| x.eq_ignore_ascii_case("inf"))
                    .unwrap_or(false)
                {
                    out.push(p);
                }
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, &mut v);
    v
}

/// Install every .inf under `pkg_dir`, printing progress to stdout.
pub fn install_dir(pkg_dir: &Path, dry_run: bool) -> Result<()> {
    install_dir_logged(pkg_dir, dry_run, &mut |line| println!("{line}"))
}

/// Install every .inf under `pkg_dir`, sending each progress line to `log`.
/// Uses `DiInstallDriverW`; the CLI plugs in `println!`, the GUI a log sink.
pub fn install_dir_logged(
    pkg_dir: &Path,
    dry_run: bool,
    log: &mut dyn FnMut(&str),
) -> Result<()> {
    let infs = find_infs(pkg_dir);
    if infs.is_empty() {
        bail!("no .inf found under {}", pkg_dir.display());
    }
    for inf in &infs {
        log(&format!("DiInstallDriver: {}", inf.display()));
        if dry_run {
            continue;
        }
        let wide: Vec<u16> = inf.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut reboot = BOOL(0);
        // Flags 0 => install to matching present devices (like pnputil /install).
        let res = unsafe {
            DiInstallDriverW(
                HWND::default(),
                PCWSTR(wide.as_ptr()),
                DIINSTALLDRIVER_FLAGS(0),
                Some(&mut reboot),
            )
        };
        match res {
            Ok(()) => log(&format!(
                "  installed{}",
                if reboot.as_bool() { " (reboot required)" } else { "" }
            )),
            Err(e) => log(&format!("  install failed: {e}")),
        }
    }
    Ok(())
}
