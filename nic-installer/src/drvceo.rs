//! DrvCeo (驱动总裁) catalog: a current, per-OS NIC driver database, used as an
//! alternative to the 2018-era 360 `drivers.dat`/`drivers.7z`.
//!
//! Layout (extracted from the Inno Setup installer `Dcnetsingle.exe`, see
//! docs/DrvCeo-passwords-and-RE.md):
//!   <OS>\Network.Scindex  — a ZIP whose single entry `Scdrv.ScIndex` is
//!                           ZipCrypto-encrypted; decrypts to a GBK, '|'-delimited
//!                           text index (hwid -> inf path inside Network.7z).
//!   <OS>\Network.7z       — 7zAES archive of the actual driver files.
//!
//! Passwords were recovered statically from DrvCeo.exe (UPX-unpacked).

use crate::archive;
use anyhow::{bail, Context, Result};
use sevenz_rust2::{ArchiveReader, Password};
use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};

/// ZipCrypto password for `Scdrv.ScIndex`.
pub const SCINDEX_PASSWORD: &str = "Noime+QvS9BR3muvrdWy6s=CeoCN_Sc";
/// 7zAES password for `Network.7z`.
pub const NETWORK7Z_PASSWORD: &str = "Oj-lr[Qc494D]J-X@sysceo.com@noime.Com";

#[derive(Debug, Clone)]
#[allow(dead_code)] // class/os_ver kept for completeness / future filtering
pub struct Record {
    pub hwid: String,
    pub class: String,
    pub desc: String,
    /// Path of the .inf inside Network.7z, e.g. `Lan\Realtek\20251003\rt640x64.inf`.
    pub inf_path: String,
    pub os_ver: String,
    pub date: String,
    pub ver: String,
}

impl Record {
    /// The package directory inside Network.7z (the .inf's parent), '/'-normalized.
    pub fn package_dir(&self) -> String {
        let norm = self.inf_path.replace('\\', "/");
        match norm.rsplit_once('/') {
            Some((dir, _)) => dir.to_string(),
            None => String::new(),
        }
    }
}

/// A loaded index: records plus an uppercased hwid -> record lookup.
pub struct Index {
    pub records: Vec<Record>,
    by_hwid: HashMap<String, usize>,
}

impl Index {
    /// Probe `hwids` (most-specific first) for the first exact match.
    pub fn match_one(&self, hwids: &[String]) -> Option<&Record> {
        for h in hwids {
            if let Some(&i) = self.by_hwid.get(&h.to_ascii_uppercase()) {
                return Some(&self.records[i]);
            }
        }
        None
    }
}

/// Decrypt + parse `Network.Scindex` from a file.
pub fn load_index(scindex: &Path, password: &str) -> Result<Index> {
    let file = std::fs::File::open(scindex)
        .with_context(|| format!("opening {}", scindex.display()))?;
    load_index_reader(file, password)
}

/// Decrypt + parse the Scindex from in-memory bytes (embedded build).
pub fn load_index_bytes(bytes: &[u8], password: &str) -> Result<Index> {
    load_index_reader(Cursor::new(bytes), password)
}

/// Decrypt + parse `Scdrv.ScIndex` from any reader.
pub fn load_index_reader<R: Read + Seek>(reader: R, password: &str) -> Result<Index> {
    let mut zip = zip::ZipArchive::new(reader).context("reading Scindex zip")?;
    let mut entry = zip
        .by_index_decrypt(0, password.as_bytes())
        .context("decrypting Scdrv.ScIndex (wrong Scindex password?)")?;
    let mut raw = Vec::new();
    entry.read_to_end(&mut raw)?;
    drop(entry);

    // The index is GBK (GB18030 superset) text, '|'-delimited, one record per line.
    let (text, _, _) = encoding_rs::GB18030.decode(&raw);

    let mut records = Vec::new();
    let mut by_hwid = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        // Leading '|' -> first split field is empty; fields are 1-based below.
        let f: Vec<&str> = line.split('|').collect();
        // |[1]HWID|[2]ClassGUID|[3]Class||[5]Desc|[6]infPath|[7]Models|[8]OSdeco|[9]OSver|[10]Date|[11]Ver|...
        if f.len() < 7 {
            continue;
        }
        let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
        let rec = Record {
            hwid: get(1),
            class: get(3),
            desc: get(5),
            inf_path: get(6),
            os_ver: get(9),
            date: get(10),
            ver: get(11),
        };
        if rec.hwid.is_empty() || rec.inf_path.is_empty() {
            continue;
        }
        by_hwid
            .entry(rec.hwid.to_ascii_uppercase())
            .or_insert(records.len());
        records.push(rec);
    }
    if records.is_empty() {
        bail!("no records parsed from Scindex");
    }
    Ok(Index { records, by_hwid })
}

/// Extract one driver package (the .inf's directory) from a `Network.7z` file.
pub fn extract_driver(
    network7z: &Path,
    password: &str,
    inf_path: &str,
    dest: &Path,
) -> Result<PathBuf> {
    let reader = ArchiveReader::open(network7z, Password::from(password))
        .with_context(|| format!("opening {} (wrong Network.7z password?)", network7z.display()))?;
    extract_driver_with(reader, inf_path, dest)
}

/// Extract a driver package from `Network.7z` bytes (embedded build).
pub fn extract_driver_bytes(
    bytes: &[u8],
    password: &str,
    inf_path: &str,
    dest: &Path,
) -> Result<PathBuf> {
    let reader = ArchiveReader::new(Cursor::new(bytes), Password::from(password))
        .context("opening embedded Network.7z")?;
    extract_driver_with(reader, inf_path, dest)
}

/// Extract the directory containing `inf_path` from an already-opened 7z reader.
pub fn extract_driver_with<R: Read + Seek>(
    reader: ArchiveReader<R>,
    inf_path: &str,
    dest: &Path,
) -> Result<PathBuf> {
    let norm = inf_path.replace('\\', "/");
    let dir = norm.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    if dir.is_empty() {
        bail!("driver inf path has no directory: {inf_path}");
    }
    archive::extract_with(reader, &dir, dest)
}

/// Resolve the per-OS folder under a DrvCeo `app` tree. If `base` already holds a
/// `Network.Scindex`, it's used directly; otherwise `<base>/<WinNNxAA>`.
pub fn resolve_os_dir(base: &Path, os_tag: &str, pf_tag: &str) -> PathBuf {
    if base.join("Network.Scindex").exists() {
        return base.to_path_buf();
    }
    base.join(os_folder(os_tag, pf_tag))
}

/// Map host OS/arch tags to a DrvCeo folder name (Win10x64, Win7x86, WinXPx86, …).
pub fn os_folder(os_tag: &str, pf_tag: &str) -> String {
    let base = match os_tag {
        "[10.0]" => "Win10",
        "[6.3]" => "Win8.1",
        "[6.2]" => "Win8",
        "[6.1]" | "[6.0]" => "Win7",
        "[5.1]" | "[5.2]" => "WinXP",
        _ => "Win10",
    };
    if base == "WinXP" {
        return "WinXPx86".to_string(); // DrvCeo only ships x86 for XP
    }
    let arch = if pf_tag == "[x64]" { "x64" } else { "x86" };
    format!("{base}{arch}")
}
