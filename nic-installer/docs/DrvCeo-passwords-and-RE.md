# Driver-catalog passwords & reverse-engineering guide

This documents the archive passwords used by the two driver tools this project
consumes, and the exact static-RE method used to recover them — so it can be
reproduced (e.g. when a newer build rotates a password).

> Scope: this is defensive/interoperability RE of locally-present installers to
> read their **driver database** (hardware-ID → driver mappings) and the
> publicly-redistributable vendor drivers they bundle. No DRM/licensing is
> bypassed.

## Passwords (verified)

| Tool | Archive | Scheme | Password |
|------|---------|--------|----------|
| 360 Driver Master | `drivers.7z` | 7zAES (solid, `7zAES:19`) | `360Drvmgr` |
| DrvCeo (驱动总裁) | `<OS>\Network.Scindex` → entry `Scdrv.ScIndex` | ZipCrypto (deflate + traditional PKWARE) | `Noime+QvS9BR3muvrdWy6s=CeoCN_Sc` |
| DrvCeo (驱动总裁) | `<OS>\Network.7z` | 7zAES (header-encrypted) | `Oj-lr[Qc494D]J-X@sysceo.com@noime.Com` |

`6AMsORoDO1vI` is **not** an archive password — it's an internal API auth token
that `sczipx*.dll!Ceo7zExtract` checks on its last argument.

## 360 Driver Master — how the password was found

1. `drivers.7z` is `7zAES` (only data encrypted; entry names cleartext):
   `7z l -slt drivers.7z`.
2. `7za.dll` exports a single `X7za` (a 7zr command-line wrapper). The caller is
   `DrvmgrNetInstaller.exe` (offline NIC installer).
3. In IDA, the wrapper `sub_40A030` builds `x "<archive>" -y -o"<dir>" -p%s` and
   appends the password from a config struct. Tracing the struct back through
   `sub_402830` → `sub_402420` shows member 4 is assigned the literal
   `L"360Drvmgr"`.
4. Verify: `7z t drivers.7z -p360Drvmgr` → "Everything is Ok".

(See also memory: `drivers-7z-password`, `nic-detection-and-install-flow`.)

## DrvCeo — how the passwords were found

DrvCeo's offline NIC installers (`Dcnetsingle.exe` / `DrvCeonwinstaller.exe`,
~663 MB) are **Inno Setup 5.6.0 (unicode)** self-extractors. Steps:

### 1. Identify & extract the installer (offline, no execution)

- PE has a tiny 264 KB image + a ~662 MB overlay starting with magic
  `7A 6C 62 1A` (`zlb\x1A` = Inno Setup's `TCompressedBlockReader` magic).
- The entry point creates an `InnoSetupLdrWindow` and the binary contains
  `Inno Setup Setup Data (5.5.7) (u)` — confirming Inno Setup.
- Extract with **innoextract** (static, does not run the installer):

  ```sh
  innoextract -l Dcnetsingle.exe                 # list
  innoextract -d out Dcnetsingle.exe             # full extract
  innoextract -I Win10x64 -d out Dcnetsingle.exe # just one OS set
  ```

  Yields `app\<OS>\Network.Scindex` (index) + `app\<OS>\Network.7z` (drivers),
  plus the app binaries under `app\` and `app\Res\`.

### 2. Get to analyzable code

- `app\DrvCeo.exe` and the `Dc*x64.exe` helpers are **UPX-packed**
  (`UPX0`/`UPX1` sections, IAT hidden → IDA shows 0 imports). Unpack:

  ```sh
  upx -d DrvCeo.exe
  ```

  After unpacking, strings/imports are restored. `dcairbx86.dll` ships unpacked.
- The 7z engine is `app\Res\sczipx86.dll` / `sczipx64.dll` — a statically-linked
  7-Zip exporting `Ceo7zExtract` / `Ceo7zCompress`. `Ceo7zExtract(archive,
  password, outdir, …, authToken)` builds `x <archive> -o<dir> … -p<password>`
  and requires `authToken == "6AMsORoDO1vI"`.

### 3. Recover the passwords (in IDA, on unpacked DrvCeo.exe)

The passwords are hardcoded UTF-16 constants, found by their *neighbours*:

- **Index (ZipCrypto) password** sits next to every `Scdrv.ScIndex` reference:
  `Noime+QvS9BR3muvrdWy6s=CeoCN_Sc`.
- **Network.7z password** sits beside the install/decode strings
  (`Load7zApi`, `Decodeing`), at roughly file offset `0x5fa62c`:
  `Oj-lr[Qc494D]J-X@sysceo.com@noime.Com`. (Note the `[`, `]`, `@`, `.` — a
  naïve "base64-only" string filter misses it; widen the charset.)

Quick neighbour-dump recipe (Python, on the unpacked exe):

```python
d = open("DrvCeo.exe", "rb").read()
# collect (offset, utf16le-string) pairs, then print strings adjacent to
# anchors like "Scdrv.ScIndex", "Load7zApi", "Decodeing", ".7z".
```

### 4. Verify

```sh
# Index (ZipCrypto) — Python:
python -c "import zipfile;print(len(zipfile.ZipFile('Network.Scindex').read('Scdrv.ScIndex',pwd=b'Noime+QvS9BR3muvrdWy6s=CeoCN_Sc')))"
# Drivers (7zAES):
7z l Network.7z -p"Oj-lr[Qc494D]J-X@sysceo.com@noime.Com"
```

## DrvCeo index format (`Scdrv.ScIndex`)

GBK (GB18030) text, one record per line, `|`-delimited:

```
|<HWID>|<ClassGUID>|<Class>||<Description>|<inf-path-in-Network.7z>|<Models>|<TargetOSdeco>|<OSver>|<DriverDate>|<DriverVer>|Network|local
```

Example:

```
|PCI\VEN_10EC&DEV_8126&REV_00|{4d36e972-...}|Net||Realtek PCIe 5GbE Family Controller|Lan\Realtek\20251003\rt640x64.inf|Realtek.NTamd64.10.0|NTamd64|10.0|10/03/2025|10.079.50.1003|Network|local
```

Matching is exact on `<HWID>` (uppercased), probing the device's hardware IDs
most-specific-first. The winning record's inf path locates the driver's folder
inside `Network.7z`. The catalog is per-OS (`Win10x64`, `Win7x86`, `WinXPx86`, …),
so OS/arch selection is folder selection.

## Using it from nic-installer

```sh
nic-installer match   --drvceo <app\Win10x64> --hwid "PCI\VEN_10EC&DEV_8126"
nic-installer extract "Lan/Realtek/20251003" --drvceo <app\Win10x64> --out ./out
nic-installer install --drvceo <app\Win10x64> --all        # elevated
```

Passwords are baked into `src/drvceo.rs` (`SCINDEX_PASSWORD`,
`NETWORK7Z_PASSWORD`); if a future DrvCeo build rotates them, repeat §3 and update
those two constants.
