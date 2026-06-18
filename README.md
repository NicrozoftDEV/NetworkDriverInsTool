# NetworkDriverInsTool

Offline **network-driver installer** for Windows. Detect the machine's network
controllers, match them against an offline driver catalog, extract the matching
driver, and install it — entirely offline, no internet required.

Two front-ends, one core:

| Crate | What |
|-------|------|
| [`nic-installer`](nic-installer/) | Core library **+ CLI**. Detection, catalog lookup, 7z/zip extraction, install. |
| [`nzndit`](nzndit/) | **NZNDIT** — *Nicrozoft Network Driver Install Tool*, a small native GUI. |

Reverse-engineered from two Chinese offline driver tools; methodology and the
recovered archive passwords are documented in
[`nic-installer/docs/DrvCeo-passwords-and-RE.md`](nic-installer/docs/DrvCeo-passwords-and-RE.md).

## How it works

1. **Detect** — SetupAPI enumerates present devices and keeps network controllers
   (PCI class `CC_0200`/`CC_0280`), reading their hardware IDs.
2. **Match** — look each hardware ID up (most-specific first) in a catalog:
   - **DrvCeo** (current): `Network.Scindex` (ZipCrypto index) → hwid→inf map, with
     drivers in `Network.7z` (7zAES).
   - **360 Driver Master** (2018): `drivers.dat` (SQLite) + `drivers.7z` (7zAES),
     optionally baked into the binary (`--features embedded`).
3. **Extract** the matching driver package from the archive.
4. **Install** it with the Windows `DiInstallDriverW` API (newdev.dll) — the same
   call `pnputil /add-driver … /install` makes; no external process.

## NZNDIT (GUI)

A deliberately small native GUI (built on [native-windows-gui], thin Win32
wrapper — no GPU, no web runtime; ~1.9 MB release binary). It's
**Per-Monitor-V2 DPI aware**, so controls and fonts scale crisply on high-DPI
displays. Pick a **catalog** (DrvCeo or 360) with the radio buttons, **Scan** to
list detected NICs and their matched drivers, select one, and **Install selected
driver**. Extraction/installation runs on a worker thread, so the window stays
responsive; progress streams into the log. Installing requires running NZNDIT
**as Administrator**.

Two build modes:

| Build | Catalogs | Folder? |
|-------|----------|---------|
| `cargo build --release -p nzndit` | DrvCeo and 360, from a folder you pick | yes — DrvCeo `app\Win10x64`, or a 360 dir with `drivers.dat`+`drivers.7z` |
| `cargo build --release -p nzndit --features embedded` | **both DrvCeo and 360 baked into the EXE** | **none — fully standalone** |

The embedded build `include_bytes!`-es both catalogs (the EXE is ~530 MB); the
folder field is disabled and Scan reads the built-in catalog you selected. The
DrvCeo assets for the embedded build live in `nzndit/assets/drvceo/`
(`Network.Scindex` + `Network.7z`, gitignored — copy them in before building).

## Build

```powershell
cargo build --release
# -> target\release\nzndit.exe          (GUI)
# -> target\release\nic-installer.exe   (CLI)
```

## CLI quick reference

```powershell
nic-installer detect [--all]
nic-installer match   --drvceo <app\Win10x64> --hwid "PCI\VEN_10EC&DEV_8126"
nic-installer extract "Lan/Realtek/20251003" --drvceo <app\Win10x64> --out .\out
nic-installer install --drvceo <app\Win10x64> --all          # elevated
```

Without `--drvceo` it uses the 360 catalog (`drivers.dat`/`drivers.7z`, or the
embedded bundle if built with `--features embedded`). See
[`nic-installer/README.md`](nic-installer/README.md) for full options.

## License

GPL-3.0-only. See [LICENSE](LICENSE). © NicrozoftDEV.

Repository: <https://github.com/NicrozoftDEV/NetworkDriverInsTool>

[native-windows-gui]: https://github.com/gabdube/native-windows-gui
