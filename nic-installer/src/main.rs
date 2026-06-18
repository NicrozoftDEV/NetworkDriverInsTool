// SPDX-License-Identifier: GPL-3.0-only
//! nic-installer — offline network-driver installer for the 360 Driver Master
//! driver set, reverse-engineered from DrvmgrNetInstaller.exe.
//!
//! Pipeline (mirrors the original /all and /hid flows):
//!   detect  : enumerate present network controllers via SetupAPI
//!   match   : look up each hardware ID in drivers.dat (t_hidandpkg) filtered by
//!             OS version + platform
//!   install : extract the matching package and install its INF(s) with pnputil
//!
//! Data source: by default the external `drivers.dat` + `drivers.7z` next to the
//! binary; when built with `--features embedded`, both are baked into the EXE and
//! used automatically (CLI path overrides still win).

use nic_installer::{archive, db, detect, drvceo, install, sysinfo};
#[cfg(feature = "embedded")]
use nic_installer::embedded;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "nic-installer",
    version,
    about = "Offline network-driver installer (360 Driver Master driver set)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enumerate present network controllers (and whether each needs a driver).
    Detect {
        /// Show all NICs, not only those currently missing a driver.
        #[arg(long)]
        all: bool,
    },
    /// Look up matching driver package(s) in the catalog.
    Match {
        /// Hardware ID to look up (repeatable). Omit to use detected NICs.
        #[arg(long = "hwid")]
        hwids: Vec<String>,
        /// drivers.dat path (default: embedded if built in, else ./drivers.dat).
        #[arg(long)]
        data: Option<PathBuf>,
        /// Use the DrvCeo catalog instead: path to its `app` tree or an OS folder
        /// (the one holding Network.Scindex + Network.7z).
        #[arg(long)]
        drvceo: Option<PathBuf>,
    },
    /// Extract a single package: a 360 PATH hash, or (with --drvceo) a directory
    /// prefix inside Network.7z such as `Lan/Realtek/20251003`.
    Extract {
        /// 360 package hash, or a DrvCeo Network.7z directory prefix.
        path: String,
        /// Source archive (default: embedded if built in, else ./drivers.7z).
        #[arg(long)]
        archive: Option<PathBuf>,
        #[arg(long, default_value = "360Drvmgr")]
        password: String,
        /// Extract from the DrvCeo catalog at this `app`/OS dir instead.
        #[arg(long)]
        drvceo: Option<PathBuf>,
        #[arg(long, default_value = "extracted")]
        out: PathBuf,
    },
    /// Detect -> match -> extract -> install.
    Install {
        /// Target a specific hardware ID (repeatable). Omit to use detected NICs.
        #[arg(long = "hwid")]
        hwids: Vec<String>,
        /// Target all detected NICs (default: only those missing a driver).
        #[arg(long)]
        all: bool,
        #[arg(long)]
        data: Option<PathBuf>,
        #[arg(long)]
        archive: Option<PathBuf>,
        #[arg(long, default_value = "360Drvmgr")]
        password: String,
        /// Use a pre-extracted package tree instead of an archive (fast path).
        #[arg(long)]
        drivers_dir: Option<PathBuf>,
        /// Use the DrvCeo catalog instead: path to its `app` tree or an OS folder.
        #[arg(long)]
        drvceo: Option<PathBuf>,
        /// Directory to extract packages into.
        #[arg(long, default_value = "extracted")]
        extract_to: PathBuf,
        /// Print the plan without extracting or installing.
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Detect { all } => cmd_detect(all),
        Cmd::Match { hwids, data, drvceo } => cmd_match(hwids, data, drvceo),
        Cmd::Extract {
            path,
            archive,
            password,
            drvceo,
            out,
        } => {
            let dir = if let Some(dc) = &drvceo {
                let os = sysinfo::os_tag();
                let net7z = drvceo::resolve_os_dir(dc, &os, sysinfo::pf_tag()).join("Network.7z");
                let reader = sevenz_rust2::ArchiveReader::open(
                    &net7z,
                    sevenz_rust2::Password::from(drvceo::NETWORK7Z_PASSWORD),
                )?;
                archive::extract_with(reader, &path.replace('\\', "/"), &out)?
            } else {
                provide_package(&None, &archive, &password, &path, &out)?
            };
            println!("extracted to {}", dir.display());
            Ok(())
        }
        Cmd::Install {
            hwids,
            all,
            data,
            archive,
            password,
            drivers_dir,
            drvceo,
            extract_to,
            dry_run,
        } => cmd_install(InstallArgs {
            hwids,
            all,
            data,
            archive,
            password,
            drivers_dir,
            drvceo,
            extract_to,
            dry_run,
        }),
    }
}

fn cmd_detect(all: bool) -> Result<()> {
    let nics = detect::enumerate_nics()?;
    let shown: Vec<_> = nics.iter().filter(|n| all || n.needs_driver()).collect();
    if shown.is_empty() {
        println!(
            "No network controllers {}.",
            if all { "found" } else { "missing a driver" }
        );
        return Ok(());
    }
    for n in shown {
        println!(
            "{}  {}",
            if n.needs_driver() {
                "[needs driver]"
            } else {
                "[   ok      ]"
            },
            n.description
        );
        if n.problem != 0 {
            println!("    CM problem code: {}", n.problem);
        }
        if let Some(primary) = n.hardware_ids.first() {
            println!("    hwid: {primary}");
        }
        for id in n.hardware_ids.iter().skip(1) {
            println!("          {id}");
        }
    }
    Ok(())
}

fn cmd_match(hwids: Vec<String>, data: Option<PathBuf>, drvceo: Option<PathBuf>) -> Result<()> {
    let os = sysinfo::os_tag();
    let pf = sysinfo::pf_tag();
    println!("host: OS {os}  PF {pf}");
    let jobs = build_jobs(&hwids, true)?;

    if let Some(dc) = &drvceo {
        let osdir = drvceo::resolve_os_dir(dc, &os, pf);
        println!("catalog: DrvCeo @ {}", osdir.display());
        let index = drvceo::load_index(&osdir.join("Network.Scindex"), drvceo::SCINDEX_PASSWORD)?;
        for (label, ids) in jobs {
            println!("== {label} ==");
            match index.match_one(&ids) {
                Some(r) => println!(
                    "  {}\n    -> {} [{}]  ({} {})",
                    r.hwid, r.inf_path, r.desc, r.date, r.ver
                ),
                None => println!("  no DrvCeo match"),
            }
        }
        return Ok(());
    }

    let dbh = open_store_db(&data)?;
    for (label, ids) in jobs {
        println!("== {label} ==");
        match db::match_one(dbh.conn(), &ids, &os, pf)? {
            Some((hit, pkg)) => println!(
                "  {hit}\n    -> package {} [{}]  (OS {} PF {} TYPE {})",
                pkg.path, pkg.hidname, pkg.os, pkg.pf, pkg.kind
            ),
            None => println!("  no match for OS {os} {pf}"),
        }
    }
    Ok(())
}

struct InstallArgs {
    hwids: Vec<String>,
    all: bool,
    data: Option<PathBuf>,
    archive: Option<PathBuf>,
    password: String,
    drivers_dir: Option<PathBuf>,
    drvceo: Option<PathBuf>,
    extract_to: PathBuf,
    dry_run: bool,
}

fn cmd_install(a: InstallArgs) -> Result<()> {
    let os = sysinfo::os_tag();
    let pf = sysinfo::pf_tag();
    println!("host: OS {os}  PF {pf}");

    let jobs = build_jobs(&a.hwids, a.all)?;
    if jobs.is_empty() {
        println!("No target NICs (everything already has a driver? try --all).");
        return Ok(());
    }
    if !a.dry_run && !sysinfo::is_admin() {
        bail!("Administrator rights are required to install drivers. Re-run from an elevated prompt.");
    }

    if a.drvceo.is_some() {
        return install_drvceo(&a, &os, pf, jobs);
    }

    let dbh = open_store_db(&a.data)?;
    for (label, ids) in jobs {
        println!("== {label} ==");
        let (hit, pkg) = match db::match_one(dbh.conn(), &ids, &os, pf)? {
            Some(x) => x,
            None => {
                println!("  no matching driver package for OS {os} {pf}");
                continue;
            }
        };
        println!("  match {hit} -> package {} [{}]", pkg.path, pkg.hidname);

        let pkg_dir = match (&a.drivers_dir, a.dry_run) {
            (Some(dir), _) => dir.join(&pkg.path),
            (None, true) => a.extract_to.join(&pkg.path), // not extracted in dry-run
            (None, false) => {
                println!("  extracting package {} ...", pkg.path);
                provide_package(&None, &a.archive, &a.password, &pkg.path, &a.extract_to)?
            }
        };

        if a.dry_run && !pkg_dir.exists() {
            println!("  would extract package {} and install its INF(s)", pkg.path);
            continue;
        }
        install::install_dir(&pkg_dir, a.dry_run)?;
    }
    Ok(())
}

/// Install path using the DrvCeo catalog (Network.Scindex + Network.7z).
fn install_drvceo(
    a: &InstallArgs,
    os: &str,
    pf: &str,
    jobs: Vec<(String, Vec<String>)>,
) -> Result<()> {
    let base = a.drvceo.as_ref().unwrap();
    let osdir = drvceo::resolve_os_dir(base, os, pf);
    let scindex = osdir.join("Network.Scindex");
    let net7z = osdir.join("Network.7z");
    println!("catalog: DrvCeo @ {}", osdir.display());
    let index = drvceo::load_index(&scindex, drvceo::SCINDEX_PASSWORD)?;

    for (label, ids) in jobs {
        println!("== {label} ==");
        let rec = match index.match_one(&ids) {
            Some(r) => r.clone(),
            None => {
                println!("  no DrvCeo match");
                continue;
            }
        };
        println!(
            "  {} -> {} [{}] ({} {})",
            rec.hwid, rec.inf_path, rec.desc, rec.date, rec.ver
        );
        if a.dry_run {
            println!(
                "  would extract {} from Network.7z and install {}",
                rec.package_dir(),
                rec.inf_path
            );
            continue;
        }
        println!("  extracting {} from Network.7z ...", rec.package_dir());
        let pkg_dir = drvceo::extract_driver(
            &net7z,
            drvceo::NETWORK7Z_PASSWORD,
            &rec.inf_path,
            &a.extract_to,
        )?;
        install::install_dir(&pkg_dir, a.dry_run)?;
    }
    Ok(())
}

/// Build (label, id-list) work items. With explicit hwids, one job per id;
/// otherwise enumerate NICs (optionally only those missing a driver).
fn build_jobs(hwids: &[String], all: bool) -> Result<Vec<(String, Vec<String>)>> {
    if !hwids.is_empty() {
        return Ok(hwids.iter().map(|h| (h.clone(), vec![h.clone()])).collect());
    }
    let mut jobs = Vec::new();
    for n in detect::enumerate_nics()? {
        if !all && !n.needs_driver() {
            continue;
        }
        let label = if n.description.is_empty() {
            n.hardware_ids
                .first()
                .cloned()
                .unwrap_or_else(|| "<unknown NIC>".into())
        } else {
            n.description.clone()
        };
        jobs.push((label, n.match_ids()));
    }
    Ok(jobs)
}

/// Open drivers.dat: explicit `--data` wins; else the embedded copy (if built
/// in); else `./drivers.dat`.
fn open_store_db(data: &Option<PathBuf>) -> Result<db::DbHandle> {
    if let Some(p) = data {
        return Ok(db::DbHandle::new(db::open(p)?, None));
    }
    #[cfg(feature = "embedded")]
    {
        let (conn, temp) = embedded::open_db()?;
        return Ok(db::DbHandle::new(conn, Some(temp)));
    }
    #[cfg(not(feature = "embedded"))]
    Ok(db::DbHandle::new(db::open(Path::new("drivers.dat"))?, None))
}

/// Materialize a package: `--drivers-dir` wins; else `--archive`; else the
/// embedded bundle (if built in); else `./drivers.7z`.
fn provide_package(
    drivers_dir: &Option<PathBuf>,
    archive: &Option<PathBuf>,
    password: &str,
    pkg_path: &str,
    extract_to: &Path,
) -> Result<PathBuf> {
    if let Some(dir) = drivers_dir {
        return Ok(dir.join(pkg_path));
    }
    if let Some(arc) = archive {
        return archive::extract_package(arc, password, pkg_path, extract_to);
    }
    #[cfg(feature = "embedded")]
    {
        return embedded::extract_package(pkg_path, extract_to);
    }
    #[cfg(not(feature = "embedded"))]
    archive::extract_package(Path::new("drivers.7z"), password, pkg_path, extract_to)
}
