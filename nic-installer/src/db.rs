//! drivers.dat lookup — a port of the query in `sub_4053F0`:
//!
//!   SELECT HID, HIDNAME, PATH, OS, PF, TYPE FROM t_hidandpkg
//!   WHERE HID = '<UPPERCASED HWID>' AND OS LIKE '%[maj.min]%' AND PF LIKE '%[xNN]%'
//!
//! The hardware ID is upper-cased before matching (original calls _wcsupr_s),
//! and OS/PF are matched as substrings because a single row's OS column may
//! list several versions, e.g. "[5.1][5.2][6.0][6.1]".

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// Owns a DB connection plus an optional temp file (when the DB was materialized
/// from the embedded bundle). Drops the connection before deleting the temp file
/// so Windows lets us remove it.
pub struct DbHandle {
    conn: Option<Connection>,
    temp: Option<PathBuf>,
}

impl DbHandle {
    pub fn new(conn: Connection, temp: Option<PathBuf>) -> Self {
        Self {
            conn: Some(conn),
            temp,
        }
    }
    pub fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("connection present")
    }
}

impl Drop for DbHandle {
    fn drop(&mut self) {
        self.conn.take(); // close the DB first
        if let Some(p) = self.temp.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    /// The HID value as stored in the DB (the canonical matched hardware ID).
    #[allow(dead_code)]
    pub hid: String,
    pub hidname: String,
    /// MD5-style folder name inside drivers.7z holding the driver files.
    pub path: String,
    pub os: String,
    pub pf: String,
    pub kind: String,
}

pub fn open(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

/// Probe `hwids` in order; return the first matching package (and the ID that hit).
pub fn match_one(
    conn: &Connection,
    hwids: &[String],
    os_tag: &str,
    pf_tag: &str,
) -> Result<Option<(String, Package)>> {
    let mut stmt = conn.prepare(
        "SELECT HID, HIDNAME, PATH, OS, PF, TYPE FROM t_hidandpkg \
         WHERE HID = ?1 AND OS LIKE ?2 AND PF LIKE ?3 LIMIT 1",
    )?;
    let os_like = format!("%{os_tag}%");
    let pf_like = format!("%{pf_tag}%");

    for hw in hwids {
        let key = hw.to_ascii_uppercase();
        let mut rows = stmt.query(rusqlite::params![key, os_like, pf_like])?;
        if let Some(r) = rows.next()? {
            let pkg = Package {
                hid: r.get(0)?,
                hidname: r.get(1)?,
                path: r.get(2)?,
                os: r.get(3)?,
                pf: r.get(4)?,
                kind: r.get::<_, String>(5).unwrap_or_default(),
            };
            return Ok(Some((hw.clone(), pkg)));
        }
    }
    Ok(None)
}
