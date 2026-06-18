//! Extract one driver package from a 7z by streaming the whole archive once.
//!
//! Used for both the external `drivers.7z` (AES, solid; password `360Drvmgr`)
//! and the embedded bundle's solid package block. The archive is a solid block,
//! so the LZMA stream must be decoded sequentially and every entry drained to
//! stay aligned — we keep only the files belonging to the requested package.

use anyhow::{bail, Context, Result};
use sevenz_rust2::{ArchiveReader, Password};
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

/// Open an on-disk 7z and extract `pkg_path` into `dest`.
pub fn extract_package(
    archive: &Path,
    password: &str,
    pkg_path: &str,
    dest: &Path,
) -> Result<PathBuf> {
    let reader = ArchiveReader::open(archive, Password::from(password))
        .with_context(|| format!("opening archive {}", archive.display()))?;
    extract_with(reader, pkg_path, dest)
}

/// Stream `reader` once, writing the files of `pkg_path` into `dest/<pkg_path>`.
/// Generic over the source so the embedded `Cursor<&[u8]>` reuses it.
pub fn extract_with<R: Read + Seek>(
    mut reader: ArchiveReader<R>,
    pkg_path: &str,
    dest: &Path,
) -> Result<PathBuf> {
    let prefix = format!("{pkg_path}/"); // entry names normalized to '/' below
    let mut wrote = 0usize;
    let mut io_error: Option<anyhow::Error> = None;

    reader.for_each_entries(|entry, rd| {
        let name = entry.name.replace('\\', "/");
        let rel = if name == pkg_path {
            Some(String::new())
        } else {
            name.strip_prefix(&prefix).map(str::to_owned)
        };

        let res: io::Result<()> = (|| {
            match rel {
                Some(rel) if !rel.is_empty() => {
                    let target = dest.join(pkg_path).join(&rel);
                    if entry.is_directory {
                        std::fs::create_dir_all(&target)?;
                    } else {
                        if let Some(parent) = target.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        let mut f = std::fs::File::create(&target)?;
                        io::copy(rd, &mut f)?; // reading drains this entry
                        wrote += 1;
                    }
                    Ok(())
                }
                // Not ours (or the bare package dir): drain so the solid stream
                // stays aligned for entries we DO want.
                _ => {
                    io::copy(rd, &mut io::sink())?;
                    Ok(())
                }
            }
        })();
        if let Err(e) = res {
            io_error = Some(anyhow::Error::from(e).context("extracting entry"));
            return Ok(false);
        }
        Ok(true)
    })?;

    if let Some(e) = io_error {
        return Err(e);
    }
    if wrote == 0 {
        bail!("package '{pkg_path}' not found in archive");
    }
    Ok(dest.join(pkg_path))
}
