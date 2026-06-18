//! Single-file mode: the driver store is baked into the executable as one 7z
//! (`assets/bundle.7z`) via `include_bytes!`. Only compiled with the `embedded`
//! feature.
//!
//! Layout (two blocks in one .7z):
//!   * `drivers.dat`  — its own non-solid block, so it decompresses on its own
//!                      (instant `detect`/`match`), independent of the payload.
//!   * `<hash>/...`    — all packages in one solid block (best compression).
//!
//! The bundle is unencrypted (it already lives inside our binary; AES would only
//! waste CPU). Package extraction streams the solid block once (see `archive`).

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use sevenz_rust2::{ArchiveReader, Password};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// The embedded bundle, in the binary's read-only data (mapped on demand by the
/// OS, so not all resident).
static BUNDLE: &[u8] = include_bytes!("../assets/bundle.7z");

fn reader() -> Result<ArchiveReader<Cursor<&'static [u8]>>> {
    ArchiveReader::new(Cursor::new(BUNDLE), Password::empty()).context("reading embedded bundle")
}

/// Materialize drivers.dat (its own block → cheap) to a temp file, open it
/// read-only, and return the connection plus the temp path to clean up.
pub fn open_db() -> Result<(Connection, PathBuf)> {
    let mut r = reader()?;
    let bytes = r
        .read_file("drivers.dat")
        .context("drivers.dat missing from embedded bundle")?;
    let mut path = std::env::temp_dir();
    path.push(format!("nic-installer-{}.dat", std::process::id()));
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok((conn, path))
}

/// Extract one package from the embedded solid block into `dest`.
pub fn extract_package(pkg_path: &str, dest: &Path) -> Result<PathBuf> {
    crate::archive::extract_with(reader()?, pkg_path, dest)
}
