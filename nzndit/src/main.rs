// SPDX-License-Identifier: GPL-3.0-only
//! NZNDIT — Nicrozoft Network Driver Install Tool.
//!
//! A small native GUI (native-windows-gui; thin Win32 wrapper, System-DPI aware
//! with control coordinates scaled by the `high-dpi` feature, no GPU/web runtime)
//! over the `nic_installer` library. It detects NICs, matches
//! them against a chosen catalog, extracts the matching driver and installs it via
//! the Windows `DiInstallDriverW` API.
//!
//! Two catalogs, selectable at runtime:
//!   * DrvCeo — current per-OS driver set (Network.Scindex + Network.7z).
//!   * 360    — 2018 360 Driver Master set (drivers.dat + drivers.7z).
//! Each can come from a folder, or be baked into the EXE with `--features embedded`
//! (then no folder is needed). Long work runs on a worker thread; progress streams
//! back via an nwg `Notice` so the window never freezes.
#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::NativeUi;

use nic_installer::{archive, db, detect, drvceo, install, sysinfo};

const M360_PASSWORD: &str = "360Drvmgr";

/// DrvCeo catalog assets baked into the EXE (embedded build only).
#[cfg(feature = "embedded")]
mod embed {
    use nic_installer::drvceo;
    use std::path::{Path, PathBuf};
    static SCINDEX: &[u8] = include_bytes!("../assets/drvceo/Network.Scindex");
    static NET7Z: &[u8] = include_bytes!("../assets/drvceo/Network.7z");
    pub fn drvceo_index() -> anyhow::Result<drvceo::Index> {
        drvceo::load_index_bytes(SCINDEX, drvceo::SCINDEX_PASSWORD)
    }
    pub fn drvceo_extract(inf: &str, dest: &Path) -> anyhow::Result<PathBuf> {
        drvceo::extract_driver_bytes(NET7Z, drvceo::NETWORK7Z_PASSWORD, inf, dest)
    }
}

/// A self-contained, `Send` install unit handed to the worker thread.
#[derive(Clone)]
#[allow(dead_code)] // file vs embedded variants vary by build
enum Job {
    DrvCeoFile { net7z: PathBuf, inf: String },
    M360File { archive: PathBuf, pkg: String },
    #[cfg(feature = "embedded")]
    DrvCeoEmbedded { inf: String },
    #[cfg(feature = "embedded")]
    M360Embedded { pkg: String },
}

impl Job {
    fn run(self, dest: &Path, emit: &mut dyn FnMut(String)) {
        let res: anyhow::Result<PathBuf> = match &self {
            Job::DrvCeoFile { net7z, inf } => {
                emit(format!("Extracting {inf} from Network.7z (~40 s)..."));
                drvceo::extract_driver(net7z, drvceo::NETWORK7Z_PASSWORD, inf, dest)
            }
            Job::M360File { archive, pkg } => {
                emit(format!("Extracting {pkg} from drivers.7z..."));
                archive::extract_package(archive, M360_PASSWORD, pkg, dest)
            }
            #[cfg(feature = "embedded")]
            Job::DrvCeoEmbedded { inf } => {
                emit(format!("Extracting {inf} from embedded Network.7z (~40 s)..."));
                embed::drvceo_extract(inf, dest)
            }
            #[cfg(feature = "embedded")]
            Job::M360Embedded { pkg } => {
                emit(format!("Extracting {pkg} from embedded catalog..."));
                nic_installer::embedded::extract_package(pkg, dest)
            }
        };
        match res {
            Ok(dir) => {
                emit(format!("Extracted to {}", dir.display()));
                let mut sink = |l: &str| emit(l.to_string());
                match install::install_dir_logged(&dir, false, &mut sink) {
                    Ok(()) => emit("Install finished.".into()),
                    Err(e) => emit(format!("Install error: {e}")),
                }
            }
            Err(e) => emit(format!("Extract error: {e}")),
        }
    }

    /// Serialize for the elevation hand-off file: a tag line followed by one
    /// field per line. Paths/strings never contain newlines, so this is safe.
    fn to_handoff(&self) -> String {
        match self {
            Job::DrvCeoFile { net7z, inf } => {
                format!("DrvCeoFile\n{}\n{}\n", net7z.display(), inf)
            }
            Job::M360File { archive, pkg } => {
                format!("M360File\n{}\n{}\n", archive.display(), pkg)
            }
            #[cfg(feature = "embedded")]
            Job::DrvCeoEmbedded { inf } => format!("DrvCeoEmbedded\n{inf}\n"),
            #[cfg(feature = "embedded")]
            Job::M360Embedded { pkg } => format!("M360Embedded\n{pkg}\n"),
        }
    }

    /// Reconstruct a job written by `to_handoff` in the unelevated instance.
    fn from_handoff(s: &str) -> Option<Job> {
        let mut lines = s.lines();
        match lines.next()? {
            "DrvCeoFile" => Some(Job::DrvCeoFile {
                net7z: PathBuf::from(lines.next()?),
                inf: lines.next()?.to_string(),
            }),
            "M360File" => Some(Job::M360File {
                archive: PathBuf::from(lines.next()?),
                pkg: lines.next()?.to_string(),
            }),
            #[cfg(feature = "embedded")]
            "DrvCeoEmbedded" => Some(Job::DrvCeoEmbedded {
                inf: lines.next()?.to_string(),
            }),
            #[cfg(feature = "embedded")]
            "M360Embedded" => Some(Job::M360Embedded {
                pkg: lines.next()?.to_string(),
            }),
            _ => None,
        }
    }
}

#[allow(dead_code)]
enum DcSrc {
    File(PathBuf),
    #[cfg(feature = "embedded")]
    Embedded,
}
#[allow(dead_code)]
enum M3Src {
    File(PathBuf),
    #[cfg(feature = "embedded")]
    Embedded,
}

/// A loaded catalog ready to match NICs.
enum Cat {
    DrvCeo { index: drvceo::Index, src: DcSrc },
    M360 { db: db::DbHandle, os: String, pf: &'static str, src: M3Src },
}

impl Cat {
    fn open_drvceo(_folder: &str) -> anyhow::Result<Cat> {
        #[cfg(feature = "embedded")]
        {
            return Ok(Cat::DrvCeo {
                index: embed::drvceo_index()?,
                src: DcSrc::Embedded,
            });
        }
        #[cfg(not(feature = "embedded"))]
        {
            if _folder.trim().is_empty() {
                anyhow::bail!("Pick the DrvCeo app\\Win10x64 folder.");
            }
            let osdir = drvceo::resolve_os_dir(
                Path::new(_folder.trim()),
                &sysinfo::os_tag(),
                sysinfo::pf_tag(),
            );
            let sc = osdir.join("Network.Scindex");
            if !sc.exists() {
                anyhow::bail!("Network.Scindex not found in {}", osdir.display());
            }
            Ok(Cat::DrvCeo {
                index: drvceo::load_index(&sc, drvceo::SCINDEX_PASSWORD)?,
                src: DcSrc::File(osdir.join("Network.7z")),
            })
        }
    }

    fn open_360(_folder: &str) -> anyhow::Result<Cat> {
        #[cfg(feature = "embedded")]
        {
            let (conn, temp) = nic_installer::embedded::open_db()?;
            return Ok(Cat::M360 {
                db: db::DbHandle::new(conn, Some(temp)),
                os: sysinfo::os_tag(),
                pf: sysinfo::pf_tag(),
                src: M3Src::Embedded,
            });
        }
        #[cfg(not(feature = "embedded"))]
        {
            if _folder.trim().is_empty() {
                anyhow::bail!("Pick a folder containing drivers.dat + drivers.7z.");
            }
            let dir = Path::new(_folder.trim());
            Ok(Cat::M360 {
                db: db::DbHandle::new(db::open(&dir.join("drivers.dat"))?, None),
                os: sysinfo::os_tag(),
                pf: sysinfo::pf_tag(),
                src: M3Src::File(dir.join("drivers.7z")),
            })
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Cat::DrvCeo { .. } => "DrvCeo",
            Cat::M360 { .. } => "360 Driver Master",
        }
    }
    fn count(&self) -> usize {
        match self {
            Cat::DrvCeo { index, .. } => index.records.len(),
            Cat::M360 { .. } => 0,
        }
    }

    fn match_nic(&self, hwids: &[String]) -> Option<(String, Job)> {
        match self {
            Cat::DrvCeo { index, src } => index.match_one(hwids).map(|r| {
                let job = match src {
                    DcSrc::File(n) => Job::DrvCeoFile {
                        net7z: n.clone(),
                        inf: r.inf_path.clone(),
                    },
                    #[cfg(feature = "embedded")]
                    DcSrc::Embedded => Job::DrvCeoEmbedded {
                        inf: r.inf_path.clone(),
                    },
                };
                (format!("{} ({} {})", r.inf_path, r.date, r.ver), job)
            }),
            Cat::M360 { db, os, pf, src } => match db::match_one(db.conn(), hwids, os, pf) {
                Ok(Some((_, pkg))) => {
                    let job = match src {
                        M3Src::File(a) => Job::M360File {
                            archive: a.clone(),
                            pkg: pkg.path.clone(),
                        },
                        #[cfg(feature = "embedded")]
                        M3Src::Embedded => Job::M360Embedded {
                            pkg: pkg.path.clone(),
                        },
                    };
                    Some((format!("{} [{}]", pkg.path, pkg.hidname), job))
                }
                _ => None,
            },
        }
    }
}

enum Msg {
    Log(String),
    Done,
}

#[derive(Default)]
struct State {
    jobs: Vec<Option<Job>>,
    busy: bool,
    rx: Option<mpsc::Receiver<Msg>>,
}

#[derive(Default, NwgUi)]
pub struct Nzndit {
    #[nwg_control(size: (724, 552), position: (280, 140),
        title: "NZNDIT — Nicrozoft Network Driver Install Tool", flags: "WINDOW|VISIBLE")]
    #[nwg_events( OnInit: [Nzndit::on_init], OnWindowClose: [Nzndit::on_close] )]
    window: nwg::Window,

    #[nwg_control(parent: window, text: "Catalog:", position: (10, 14), size: (55, 20))]
    cat_lbl: nwg::Label,

    #[nwg_control(parent: window, text: "DrvCeo (current)", position: (66, 12), size: (150, 22), flags: "VISIBLE|GROUP")]
    #[nwg_events( OnButtonClick: [Nzndit::on_pick_drvceo] )]
    rb_drvceo: nwg::RadioButton,

    #[nwg_control(parent: window, text: "360 (2018)", position: (224, 12), size: (130, 22))]
    #[nwg_events( OnButtonClick: [Nzndit::on_pick_360] )]
    rb_360: nwg::RadioButton,

    #[nwg_control(parent: window, text: "Folder:", position: (10, 46), size: (280, 18))]
    lbl: nwg::Label,

    #[nwg_control(parent: window, position: (10, 66), size: (502, 25))]
    path: nwg::TextInput,

    #[nwg_control(parent: window, text: "Browse…", position: (520, 65), size: (88, 27))]
    #[nwg_events( OnButtonClick: [Nzndit::on_browse] )]
    browse: nwg::Button,

    #[nwg_control(parent: window, text: "Scan", position: (616, 65), size: (96, 27))]
    #[nwg_events( OnButtonClick: [Nzndit::on_scan] )]
    scan: nwg::Button,

    #[nwg_control(parent: window, position: (10, 100), size: (702, 150))]
    list: nwg::ListBox<String>,

    #[nwg_control(parent: window, text: "Install selected driver", position: (10, 258), size: (210, 30))]
    #[nwg_events( OnButtonClick: [Nzndit::on_install] )]
    install_btn: nwg::Button,

    #[nwg_control(parent: window, position: (10, 298), size: (702, 244),
        readonly: true, flags: "VISIBLE|VSCROLL|AUTOVSCROLL")]
    log: nwg::TextBox,

    #[nwg_resource(title: "Select the catalog folder", action: nwg::FileDialogAction::OpenDirectory)]
    dialog: nwg::FileDialog,

    #[nwg_control(parent: window)]
    #[nwg_events( OnNotice: [Nzndit::on_notice] )]
    notice: nwg::Notice,

    state: RefCell<State>,
}

impl Nzndit {
    fn on_init(&self) {
        self.rb_drvceo.set_check_state(nwg::RadioButtonState::Checked);
        self.update_hint();

        // If we were relaunched elevated to perform an install, do it now and
        // skip the normal setup chatter.
        if let Some(job) = take_elevated_job() {
            self.log_line("Elevated — installing the selected driver…");
            self.start_install_job(job);
            return;
        }

        if cfg!(feature = "embedded") {
            self.log_line("Both catalogs are embedded — pick DrvCeo or 360, then Scan.");
        } else {
            self.log_line("Pick a catalog and its folder, then Scan.");
        }
        if !sysinfo::is_admin() {
            // Install can't run unelevated; clicking it will trigger a UAC prompt.
            // Flag the button with the UAC shield so that's obvious up front.
            self.set_install_shield();
            self.log_line("Note: not elevated — Install will prompt for Administrator (UAC).");
        }
    }

    /// Put the UAC shield overlay on the Install button (only meaningful while
    /// unelevated, where the action will request elevation).
    fn set_install_shield(&self) {
        use winapi::um::commctrl::BCM_SETSHIELD;
        use winapi::um::winuser::SendMessageW;
        if let Some(h) = self.install_btn.handle.hwnd() {
            unsafe { SendMessageW(h, BCM_SETSHIELD, 0, 1) };
        }
    }

    fn on_close(&self) {
        nwg::stop_thread_dispatch();
    }

    fn on_pick_drvceo(&self) {
        self.rb_drvceo.set_check_state(nwg::RadioButtonState::Checked);
        self.rb_360.set_check_state(nwg::RadioButtonState::Unchecked);
        self.update_hint();
    }
    fn on_pick_360(&self) {
        self.rb_360.set_check_state(nwg::RadioButtonState::Checked);
        self.rb_drvceo.set_check_state(nwg::RadioButtonState::Unchecked);
        self.update_hint();
    }

    fn update_hint(&self) {
        #[cfg(feature = "embedded")]
        {
            self.lbl.set_text("Folder: (using embedded catalogs — none needed)");
            self.path.set_text("(embedded)");
            self.path.set_enabled(false);
            self.browse.set_enabled(false);
        }
        #[cfg(not(feature = "embedded"))]
        {
            let dc = self.rb_drvceo.check_state() == nwg::RadioButtonState::Checked;
            self.lbl.set_text(if dc {
                "DrvCeo folder (its app\\Win10x64):"
            } else {
                "360 folder (containing drivers.dat + drivers.7z):"
            });
        }
    }

    fn selected_360(&self) -> bool {
        self.rb_360.check_state() == nwg::RadioButtonState::Checked
    }

    fn on_browse(&self) {
        if self.dialog.run(Some(&self.window)) {
            if let Ok(p) = self.dialog.get_selected_item() {
                self.path.set_text(&p.to_string_lossy());
            }
        }
    }

    fn on_scan(&self) {
        if self.state.borrow().busy {
            return;
        }
        self.list.set_collection(Vec::new());
        self.state.borrow_mut().jobs.clear();

        let folder = self.path.text();
        let catalog = if self.selected_360() {
            Cat::open_360(&folder)
        } else {
            Cat::open_drvceo(&folder)
        };
        let catalog = match catalog {
            Ok(c) => c,
            Err(e) => {
                self.log_line(&format!("Failed to open catalog: {e}"));
                return;
            }
        };
        let os = sysinfo::os_tag();
        let pf = sysinfo::pf_tag();
        if catalog.count() > 0 {
            self.log_line(&format!(
                "Host {os} {pf} — {} ({} records).",
                catalog.label(),
                catalog.count()
            ));
        } else {
            self.log_line(&format!("Host {os} {pf} — {}.", catalog.label()));
        }

        let nics = match detect::enumerate_nics() {
            Ok(n) => n,
            Err(e) => {
                self.log_line(&format!("NIC detection failed: {e}"));
                return;
            }
        };
        if nics.is_empty() {
            self.log_line("No PCI network controllers detected.");
            return;
        }
        for n in nics {
            let m = catalog.match_nic(&n.match_ids());
            let line = match &m {
                Some((desc, _)) => format!("{}  ->  {}", n.description, desc),
                None => format!("{}  ->  (no driver in catalog)", n.description),
            };
            self.list.push(line);
            self.state.borrow_mut().jobs.push(m.map(|(_, job)| job));
        }
        self.log_line("Scan complete. Select a NIC and click Install.");
    }

    fn on_install(&self) {
        if self.state.borrow().busy {
            return;
        }
        let sel = match self.list.selection() {
            Some(i) => i,
            None => {
                self.log_line("Select a NIC in the list first.");
                return;
            }
        };
        let job = match self.state.borrow().jobs.get(sel).and_then(|o| o.clone()) {
            Some(j) => j,
            None => {
                self.log_line("No catalog driver for the selected NIC.");
                return;
            }
        };
        if !sysinfo::is_admin() {
            // A process can't elevate itself in place — relaunch elevated (UAC)
            // and hand the job off so the elevated instance installs it.
            match relaunch_elevated(&job) {
                Ok(()) => self
                    .log_line("Requested Administrator rights — continue in the elevated window."),
                Err(e) => self.log_line(&format!("Elevation cancelled or failed: {e}")),
            }
            return;
        }
        self.start_install_job(job);
    }

    /// Spawn the worker thread that extracts and installs `job`, streaming
    /// progress back to the log via the `Notice`. Assumes we're elevated.
    fn start_install_job(&self, job: Job) {
        if self.state.borrow().busy {
            return;
        }
        let (tx, rx) = mpsc::channel();
        {
            let mut st = self.state.borrow_mut();
            st.rx = Some(rx);
            st.busy = true;
        }
        let sender = self.notice.sender();
        let extract_to = std::env::temp_dir().join("nzndit");

        std::thread::spawn(move || {
            let mut emit = |m: String| {
                let _ = tx.send(Msg::Log(m));
                sender.notice();
            };
            job.run(&extract_to, &mut emit);
            let _ = tx.send(Msg::Done);
            sender.notice();
        });
    }

    fn on_notice(&self) {
        let mut done = false;
        {
            let st = self.state.borrow();
            if let Some(rx) = &st.rx {
                while let Ok(m) = rx.try_recv() {
                    match m {
                        Msg::Log(s) => self.log_line(&s),
                        Msg::Done => done = true,
                    }
                }
            }
        }
        if done {
            let mut st = self.state.borrow_mut();
            st.busy = false;
            st.rx = None;
        }
    }

    fn log_line(&self, s: &str) {
        let mut t = self.log.text();
        if !t.is_empty() {
            t.push_str("\r\n");
        }
        t.push_str(s);
        self.log.set_text(&t);
        let len = self.log.len();
        self.log.set_selection(len..len);
    }
}

/// Encode `s` as a NUL-terminated UTF-16 string for the Win32 `*W` APIs.
fn to_wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Re-launch this executable elevated via `ShellExecuteW`'s `runas` verb (which
/// raises the UAC prompt), handing off `job` through a temp file so the elevated
/// instance installs it. Returns an error if the launch fails or the user
/// declines the prompt.
fn relaunch_elevated(job: &Job) -> anyhow::Result<()> {
    use winapi::um::shellapi::ShellExecuteW;
    use winapi::um::winuser::SW_SHOWNORMAL;

    let exe = std::env::current_exe()?;
    let dir = std::env::temp_dir().join("nzndit");
    std::fs::create_dir_all(&dir)?;
    let handoff = dir.join(format!("elevate-{}.job", std::process::id()));
    std::fs::write(&handoff, job.to_handoff())?;

    // Build the parameter string from the OsStr path (no lossy conversion).
    let mut params = std::ffi::OsString::from("--run-elevated \"");
    params.push(handoff.as_os_str());
    params.push("\"");

    // Temporaries live until the end of this statement, so the pointers stay valid
    // for the duration of the call.
    let ret = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            to_wide("runas").as_ptr(),
            to_wide(exe.as_os_str()).as_ptr(),
            to_wide(&params).as_ptr(),
            std::ptr::null_mut(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value <= 32 on failure (including the user
    // declining the UAC prompt).
    if (ret as usize) <= 32 {
        let _ = std::fs::remove_file(&handoff); // no elevated process will read it
        anyhow::bail!("UAC declined or relaunch failed (code {})", ret as usize);
    }
    Ok(())
}

/// If this process was relaunched with `--run-elevated <file>`, read and delete
/// the hand-off file and return the job to install. Otherwise `None`.
fn take_elevated_job() -> Option<Job> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--run-elevated") {
        return None;
    }
    let path = args.next()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Job::from_handoff(&data)
}

fn main() {
    nwg::init().expect("Failed to init native-windows-gui");
    // Without this, controls fall back to the stock DEFAULT_GUI_FONT (MS Sans
    // Serif, an aliased bitmap font) which looks low-resolution next to modern
    // apps. Segoe UI is the standard Windows UI font and is ClearType-smooth; nwg
    // applies this global default to every control that sets no font of its own,
    // and scales its size for the system DPI via the `high-dpi` feature.
    let mut font = nwg::Font::default();
    nwg::Font::builder()
        .family("Segoe UI")
        .size_absolute(12) // 9 pt at 96 DPI (the Windows default); DPI-scaled by nwg
        .build(&mut font)
        .expect("Failed to build default font");
    nwg::Font::set_global_default(Some(font));

    let _ui = Nzndit::build_ui(Default::default()).expect("Failed to build UI");
    nwg::dispatch_thread_events();
}
