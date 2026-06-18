<#
.SYNOPSIS
  Build assets\bundle.7z for the `embedded` feature: drivers.dat in its own
  non-solid block (instant detect/match) + all driver packages in one solid
  block (best compression). Run from the nic-installer crate root.
#>
param(
  [string]$DriversDir = "..\360-drvmgr-drivers",   # pre-extracted package tree (<hash>\...)
  [string]$DriversDat = "..\drivers.dat",
  [string]$SevenZip   = "C:\Program Files\7-Zip\7z.exe",
  [string]$Out        = "$PSScriptRoot\..\assets\bundle.7z"
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path $SevenZip))   { throw "7-Zip not found at $SevenZip" }
if (-not (Test-Path $DriversDir)) { throw "drivers dir not found: $DriversDir" }
if (-not (Test-Path $DriversDat)) { throw "drivers.dat not found: $DriversDat" }

New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
Remove-Item $Out -ErrorAction SilentlyContinue

$staged = Join-Path $DriversDir "drivers.dat"
Copy-Item $DriversDat $staged -Force
try {
  Push-Location $DriversDir
  # Block 0: drivers.dat alone (non-solid) -> decompresses independently.
  & $SevenZip a -t7z -m0=lzma2 -mx=9 -ms=off $Out "drivers.dat" | Out-Null
  # Block 1: every package, solid.
  & $SevenZip a -t7z -m0=lzma2 -mx=9 -ms=on -mmt=on $Out "*" "-x!drivers.dat" | Out-Null
} finally {
  Pop-Location
  Remove-Item $staged -ErrorAction SilentlyContinue
}
"{0:N0} bytes  {1}" -f (Get-Item $Out).Length, $Out
