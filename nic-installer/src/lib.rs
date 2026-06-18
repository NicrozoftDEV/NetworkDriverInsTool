// SPDX-License-Identifier: GPL-3.0-only
//! nic-installer core library: offline network-driver detection, catalog lookup
//! (360 Driver Master `drivers.dat`/`drivers.7z`, or DrvCeo `Network.Scindex` +
//! `Network.7z`), package extraction, and INF installation.
//!
//! Reused by the `nic-installer` CLI and the NZNDIT GUI.

pub mod archive;
pub mod db;
pub mod detect;
pub mod drvceo;
#[cfg(feature = "embedded")]
pub mod embedded;
pub mod install;
pub mod sysinfo;
