<#
.SYNOPSIS
    Build the NZNDIT binaries (embedded catalogs) for amd64 and arm64 and bundle
    each architecture into its own zip under dist/.

.DESCRIPTION
    Always builds with `--features embedded`, so both driver catalogs are baked
    into the executables and the resulting zips are fully self-contained (no
    catalog folder needed at runtime).

    Requires the embedded assets to be present (they are git-ignored / supplied
    at build time):
        nic-installer/assets/bundle.7z
        nzndit/assets/drvceo/Network.7z
        nzndit/assets/drvceo/Network.Scindex

    Each architecture's zip contains both binaries plus LICENSE and README.md:
        nzndit.exe          (GUI front end)
        nic-installer.exe    (CLI)

.PARAMETER Arch
    Architectures to build. Default: amd64, arm64.

.PARAMETER OutDir
    Output directory for the zips, relative to the repo root. Default: dist.

.EXAMPLE
    pwsh scripts/build-bundles.ps1
    pwsh scripts/build-bundles.ps1 -Arch amd64
#>
[CmdletBinding()]
param(
    [ValidateSet('amd64', 'arm64')]
    [string[]]$Arch = @('amd64', 'arm64'),

    [string]$OutDir = 'dist'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# Handle native (cargo/rustup) exit codes ourselves via $LASTEXITCODE rather than
# letting PowerShell 7.4+ throw on the first non-zero exit.
$PSNativeCommandUseErrorActionPreference = $false

# arch -> rust target triple. Each package is built with its own `embedded`
# feature so both produced .exe files bake in the catalogs.
$Triples  = @{ amd64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }
$Packages = @('nic-installer', 'nzndit')        # cargo package -> emits <name>.exe
$Exes     = @('nic-installer.exe', 'nzndit.exe')

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $RepoRoot
try {
    # --- preflight -----------------------------------------------------------
    foreach ($tool in 'cargo', 'rustup') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "$tool not found on PATH."
        }
    }

    $assets = @(
        'nic-installer/assets/bundle.7z',
        'nzndit/assets/drvceo/Network.7z',
        'nzndit/assets/drvceo/Network.Scindex'
    )
    $missing = $assets | Where-Object { -not (Test-Path (Join-Path $RepoRoot $_)) }
    if ($missing) {
        throw ("Embedded assets missing (required for --features embedded):`n  " +
            ($missing -join "`n  "))
    }

    # version from [workspace.package] in the root Cargo.toml
    $verLine = Select-String -Path (Join-Path $RepoRoot 'Cargo.toml') `
        -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $version = if ($verLine) { $verLine.Matches[0].Groups[1].Value } else { '0.0.0' }

    $dist = Join-Path $RepoRoot $OutDir
    New-Item -ItemType Directory -Force -Path $dist | Out-Null

    # --- build + bundle each arch -------------------------------------------
    $built = @()
    foreach ($a in $Arch) {
        $triple = $Triples[$a]
        Write-Host ""
        Write-Host "=== $a ($triple) ===" -ForegroundColor Cyan

        # Make sure the std library for this target is installed (idempotent).
        rustup target add $triple | Out-Null

        # Build each package with its own embedded feature. Two explicit calls
        # keep the package -> feature mapping unambiguous in the virtual
        # workspace; cached artifacts make the second call cheap.
        $ok = $true
        foreach ($p in $Packages) {
            cargo build --release --target $triple -p $p --features embedded
            if ($LASTEXITCODE -ne 0) { $ok = $false; break }
        }
        if (-not $ok) {
            Write-Warning ("Build failed for $a ($triple) - skipping. " +
                "(arm64 needs the 'MSVC v143 ARM64 build tools' VS component.)")
            continue
        }

        # Stage the artifacts under a named folder so the zip extracts cleanly.
        $name  = "nzndit-$version-$a"
        $stage = Join-Path $dist $name
        if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
        New-Item -ItemType Directory -Force -Path $stage | Out-Null

        $binDir = Join-Path $RepoRoot "target/$triple/release"
        foreach ($exe in $Exes) {
            $src = Join-Path $binDir $exe
            if (-not (Test-Path $src)) { throw "Expected binary not found: $src" }
            Copy-Item $src $stage
        }
        foreach ($doc in 'LICENSE', 'README.md') {
            $d = Join-Path $RepoRoot $doc
            if (Test-Path $d) { Copy-Item $d $stage }
        }

        $zip = Join-Path $dist "$name.zip"
        if (Test-Path $zip) { Remove-Item -Force $zip }
        Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal
        Remove-Item -Recurse -Force $stage

        Write-Host ("  -> {0}  ({1:N0} bytes)" -f $zip, (Get-Item $zip).Length) -ForegroundColor Green
        $built += $zip
    }

    Write-Host ""
    if ($built.Count -eq 0) { throw "No bundles were produced." }
    Write-Host "Done. Bundles:" -ForegroundColor Cyan
    $built | ForEach-Object { Write-Host "  $_" }
}
finally {
    Pop-Location
}
