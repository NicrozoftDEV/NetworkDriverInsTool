# nic-installer

An offline **network-driver installer** for the 360 Driver Master driver set,
reverse-engineered from `DrvmgrNetInstaller.exe`. Given the three data files
that ship with 360 Driver Master —

| file | role |
|------|------|
| `drivers.dat` | SQLite DB: maps hardware IDs → driver package |
| `drivers.7z`  | AES-encrypted, solid 7z holding every driver package (password `360Drvmgr`) |
| `7za.dll`     | (not needed here — we use a Rust 7z decoder) |

— it detects the machine's network controllers, finds the matching package, and
installs it.

## How hardware detection works (the original, and this port)

`DrvmgrNetInstaller.exe`'s `sub_4025C0` does exactly this, and `src/detect.rs`
mirrors it:

1. **Enumerate every present device** via SetupAPI:
   `SetupDiGetClassDevsW(NULL, NULL, NULL, DIGCF_PRESENT | DIGCF_ALLCLASSES)`,
   then loop `SetupDiEnumDeviceInfo`.
2. For each device read its IDs with `SetupDiGetDeviceRegistryPropertyW`:
   `SPDRP_HARDWAREID` (prop 1, "hid") and `SPDRP_COMPATIBLEIDS` (prop 2, "cid").
3. **Filter to network controllers** by PCI class code: keep the device only if
   an ID contains `CC_0200` (Ethernet, PCI class 02 / sub-class 00) or `CC_0280`
   (other network controller, e.g. Wi-Fi). The original compares for exact
   `PCI\CC_0200` / `PCI\CC_0280` compatible IDs.
4. **Check the device status** with `CM_Get_DevNode_Status`; a non-zero problem
   code (or `DN_HAS_PROBLEM`) means Windows has no working driver bound — those
   are the install targets. (`--all` ignores this and lists every NIC.)

The hardware ID is then upper-cased and looked up in `drivers.dat`
(`sub_4053F0`):

```sql
SELECT HID, HIDNAME, PATH, OS, PF, TYPE FROM t_hidandpkg
WHERE HID = :hwid             -- uppercased, tried most-specific first
  AND OS LIKE '%[10.0]%'      -- host OS version  ([major.minor], RtlGetVersion)
  AND PF LIKE '%[x64]%'       -- host platform    (native arch)
```

`OS`/`PF` are substring-matched because a row may list several, e.g.
`[6.2][6.3][10.0]`. The winning row's **`PATH`** is the package folder name
inside `drivers.7z` (and inside a pre-extracted tree). Each package contains a
Net-class `.inf` which we hand to `pnputil`.

## Usage

```text
nic-installer detect [--all]
nic-installer match  [--hwid <ID>]... [--data drivers.dat]
nic-installer extract <PATH-hash> [--archive drivers.7z] [--password 360Drvmgr] [--out extracted]
nic-installer install [--hwid <ID>]... [--all] [--dry-run]
                      [--data drivers.dat] [--archive drivers.7z] [--password 360Drvmgr]
                      [--drivers-dir <pre-extracted tree>] [--extract-to extracted]
```

Run from the 360 Driver Master folder (so the default `drivers.dat` /
`drivers.7z` resolve), or pass explicit paths.

### Examples

```powershell
# 1. What network cards are present, and which lack a driver?
nic-installer detect --all

# 2. Which package would a given card use on this OS?
nic-installer match --hwid "PCI\VEN_1969&DEV_1063"

# 3. Install drivers for every NIC that is missing one (needs elevation).
#    Fast path: use the already-extracted package tree instead of the .7z.
nic-installer install --all --drivers-dir 360-drvmgr-drivers

# 4. Fully self-contained: extract straight from the encrypted archive.
nic-installer install --hwid "PCI\VEN_1969&DEV_1063"

# 5. See the plan without touching the system.
nic-installer install --all --drivers-dir 360-drvmgr-drivers --dry-run
```

## Notes

- **Elevation.** `detect`/`match`/`extract` run as a normal user. `install`
  performs driver-store changes and checks for admin rights, exiting with a hint
  if not elevated. (An embedded `asInvoker` manifest stops Windows' UAC
  installer-detection from force-elevating the EXE on launch.)
- **`drivers.7z` is one solid block** and packages are interleaved, so extracting
  a single package decodes the whole stream (~30 s here) — the same cost
  `7z x archive <file>` would pay. Prefer `--drivers-dir` when you already have
  the extracted tree.
- **Install method.** `pnputil /add-driver <inf> /install` (in-box, modern);
  the original used the bundled `dpinst32/64.exe`. Some legacy packages ship
  Win9x/NT-era INFs + `INSTALL.EXE` and won't install on modern Windows.
- **ARM64.** The DB only knows `[x32]`/`[x64]`; an ARM64 host maps to `[x64]`,
  which has no usable driver there. PCI-only — virtual NICs (Hyper-V/VMware/
  VPN) are intentionally skipped, exactly like the original.

## DrvCeo catalog (current driver DB)

The bundled 360 set is from 2018 and misses recent NICs. The tool can instead
use **DrvCeo (驱动总裁)**'s far newer, per-OS catalog. Extract it once (offline,
no execution) from DrvCeo's Inno Setup installer with
[innoextract](https://constexpr.org/innoextract/):

```powershell
innoextract -I Win10x64 -d dc Dcnetsingle.exe   # -> dc\app\Win10x64\{Network.Scindex, Network.7z}
```

Then point any command at it with `--drvceo` (path to the `app` tree or a single
OS folder; the OS/arch subfolder is auto-selected from the host):

```powershell
nic-installer match   --drvceo dc\app\Win10x64 --hwid "PCI\VEN_10EC&DEV_8126"
nic-installer extract "Lan/Realtek/20251003" --drvceo dc\app\Win10x64 --out .\out
nic-installer install --drvceo dc\app\Win10x64 --all          # elevated
```

How it works: `Network.Scindex` is a ZIP whose `Scdrv.ScIndex` entry is
ZipCrypto-encrypted; it decrypts to a GBK, `|`-delimited index mapping
`HWID -> inf-path` inside `Network.7z` (7zAES). nic-installer decrypts the index,
matches the NIC's hardware IDs (most-specific first), then extracts that inf's
folder from `Network.7z` and installs it with pnputil. Both passwords are baked
into `src/drvceo.rs` and documented, with the full RE method, in
[docs/DrvCeo-passwords-and-RE.md](docs/DrvCeo-passwords-and-RE.md).

> `Network.7z` is a solid archive, so a single-package extract decodes the whole
> block (~40 s here). The index match (`match`) is instant.

## Build

```powershell
cargo build --release            # external-files mode -> target\release\nic-installer.exe
```

### Single-file (embedded) build

`--features embedded` bakes the whole driver store into the EXE via
`include_bytes!(assets/bundle.7z)`, so the binary needs **no external files**:

```powershell
scripts\make-bundle.ps1                       # builds assets\bundle.7z (~207 MB)
cargo build --release --features embedded     # -> a self-contained ~209 MB EXE
```

Measured on this driver set (1.4 GB / 2899 files):

| bundle layout            | size   | single-package extract |
|--------------------------|--------|------------------------|
| solid (one block)        | 207 MB | ~4.6 s (decode block once) |
| bounded solid 256k–4m    | 331–351 MB | fast |
| non-solid (per file)     | 338 MB | <1 s (random access) |

Bounded blocks gave **no** compression benefit here (the 4× win comes only from
one large solid window over the ~10 % cross-package duplication), so the build
uses **solid** for the smallest binary. To keep `detect`/`match` instant despite
the solid payload, `drivers.dat` is packed as its **own** non-solid block —
`read_file` decompresses just it (0.07 s) without touching the package block.
`install` streams the solid block once (~4.6 s, plain LZMA2 decode; the original
`drivers.7z`'s 31 s was AES overhead).

> Want sub-second extraction at the cost of a bigger EXE? Rebuild
> `assets/bundle.7z` non-solid (`-ms=off`) — the code path is unchanged.

Dependencies: `clap`, `anyhow`, `rusqlite` (bundled SQLite), `sevenz-rust2`
(7z + AES-256), `windows` (SetupAPI / CfgMgr32 / version), `embed-manifest`.
