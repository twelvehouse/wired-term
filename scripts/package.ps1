#Requires -Version 7
<#
.SYNOPSIS
Builds wired in release mode and packs the portable zip with every license it must ship.

.EXAMPLE
pwsh scripts/package.ps1
Writes target/dist/wired-<version>-windows-x64.zip and its .sha256 file.
#>
[CmdletBinding()]
param(
    # Use the existing target/release build instead of running cargo build.
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# The upstream zip ships no license, so the font licenses live in licenses/.
$FontUrl = 'https://github.com/yuru7/udev-gothic/releases/download/v2.2.0/UDEVGothic_NF_v2.2.0.zip'
$FontSha256 = '45faeef7b5d8bc591bcc5887a2ca0c5fb9028066f18a5a52cd6f10b7d655ba37'
$FontFiles = 'UDEVGothic*NFLG-*.ttf'

$Root = Split-Path -Parent $PSScriptRoot
$Target = Join-Path $Root 'target'
$Release = Join-Path $Target 'release'
$Ghostty = Join-Path $Root '.submodules/ghostty-wired'
$Gpui = Join-Path $Root '.submodules/gpui-pre-wired'

function Invoke-Native {
    param([string]$Exe, [string[]]$Arguments)
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Exe $($Arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

function Copy-License {
    param([string]$Source, [string]$Name)
    if (-not (Test-Path -LiteralPath $Source)) { throw "license file not found: $Source" }
    Copy-Item -LiteralPath $Source -Destination (Join-Path $Licenses $Name)
}

# Same order as the libghostty-vt build script, so the license matches the Zig that built it.
function Find-Zig {
    if ($env:ZIG) { return $env:ZIG }
    $onPath = Get-Command zig -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($onPath) { return $onPath.Source }
    $installed = Get-ChildItem -Path (Join-Path $env:LOCALAPPDATA 'Programs/zig') -Directory -Filter '*-0.17.*' -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName 'zig.exe' } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if ($installed) { return $installed }
    throw 'zig not found; install Zig 0.17.x or set ZIG'
}

Set-Location -LiteralPath $Root
$Metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$Version = ($Metadata.packages | Where-Object name -EQ 'wired-app').version

if (-not $NoBuild) {
    Invoke-Native cargo @('build', '--release', '--locked', '-p', 'wired-app')
}

$Dist = Join-Path $Target 'dist'
$Name = "wired-$Version-windows-x64"
$Stage = Join-Path $Dist $Name
$App = Join-Path $Stage 'wired'
if (Test-Path -LiteralPath $Stage) { Remove-Item -LiteralPath $Stage -Recurse -Force }
New-Item -ItemType Directory -Path $App | Out-Null

# The bundled ConPTY is required: the inbox one is much slower on scroll-heavy output.
foreach ($file in 'wired.exe', 'conpty.dll', 'OpenConsole.exe') {
    Copy-Item -LiteralPath (Join-Path $Release $file) -Destination $App
}
Copy-Item -LiteralPath (Join-Path $Release 'themes') -Destination $App -Recurse
if (-not (Test-Path -LiteralPath (Join-Path $App 'themes/LICENSE.txt'))) { throw 'themes/LICENSE.txt is missing' }

$FontCache = Join-Path $Target 'fonts-cache'
$FontZip = Join-Path $FontCache (Split-Path -Leaf $FontUrl)
New-Item -ItemType Directory -Force -Path $FontCache | Out-Null
if (-not (Test-Path -LiteralPath $FontZip)) {
    Invoke-WebRequest -Uri $FontUrl -OutFile "$FontZip.part"
    Move-Item -LiteralPath "$FontZip.part" -Destination $FontZip
}
$actual = (Get-FileHash -LiteralPath $FontZip -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $FontSha256) {
    Remove-Item -LiteralPath $FontZip
    throw "font archive checksum mismatch: $actual"
}
$FontExtract = Join-Path $FontCache 'extract'
if (Test-Path -LiteralPath $FontExtract) { Remove-Item -LiteralPath $FontExtract -Recurse -Force }
Expand-Archive -LiteralPath $FontZip -DestinationPath $FontExtract
$Fonts = Join-Path $App 'fonts'
New-Item -ItemType Directory -Path $Fonts | Out-Null
$fontFound = Get-ChildItem -Path $FontExtract -Recurse -Filter $FontFiles
if (-not $fontFound) { throw "no $FontFiles in the font archive" }
$fontFound | Copy-Item -Destination $Fonts

Copy-Item -LiteralPath (Join-Path $Root 'LICENSE') -Destination $App
Copy-Item -LiteralPath (Join-Path $Root 'THIRD-PARTY-NOTICES.md') -Destination $App
Invoke-Native cargo @('about', 'generate', '--workspace', '--locked', '--fail',
    '-c', (Join-Path $Root 'about.toml'),
    '-o', (Join-Path $App 'THIRD-PARTY-LICENSES.html'),
    (Join-Path $Root 'about.hbs'))

$Licenses = Join-Path $App 'licenses'
New-Item -ItemType Directory -Path $Licenses | Out-Null
Get-ChildItem -LiteralPath (Join-Path $Root 'licenses') -File | Copy-Item -Destination $Licenses
Copy-License (Join-Path $Ghostty 'LICENSE') 'ghostty-LICENSE.txt'
Copy-License (Join-Path $Gpui 'gpui-pre/LICENSE-APACHE') 'gpui-LICENSE-APACHE.txt'
Copy-License (Join-Path $Gpui 'gpui-pre-windows/LICENSE-SKIA') 'skia-LICENSE.txt'
Copy-License (Join-Path $Root 'crates/wired-ui/assets/profiles/LICENSE-TABBY') 'tabby-LICENSE.txt'
$assets = $Metadata.packages | Where-Object name -EQ 'gpui-kit-assets'
Copy-License (Join-Path (Split-Path -Parent $assets.manifest_path) 'LICENSE-LUCIDE') 'lucide-LICENSE.txt'

$Zig = Find-Zig
if (-not (Test-Path -LiteralPath (Join-Path $Ghostty 'zig-pkg'))) {
    Push-Location -LiteralPath $Ghostty
    try { Invoke-Native $Zig @('build', '--fetch') } finally { Pop-Location }
}
$thirdParty = Get-Content -Raw -LiteralPath (Join-Path $Ghostty 'deps/libghostty-vt-third-party.json') | ConvertFrom-Json
foreach ($package in $thirdParty.third_party | Where-Object use -EQ 'linked') {
    $dir = switch ($package.kind) {
        'zig-package' { Join-Path $Ghostty "zig-pkg/$($package.hash)" }
        'vendored' { Join-Path $Ghostty $package.path }
        default { throw "unknown package kind '$($package.kind)' for $($package.name)" }
    }
    if (-not (Test-Path -LiteralPath $dir)) { throw "$($package.name) not found at $dir; run zig build --fetch in $Ghostty" }
    $files = @(Get-ChildItem -LiteralPath $dir -File | Where-Object Name -Match '^(LICENSE|COPYING)')
    if ($files.Count -eq 0) { throw "no license file for $($package.name) in $dir" }
    foreach ($file in $files) { Copy-License $file.FullName "$($package.name)-$($file.BaseName).txt" }
}
$zigEnv = (& $Zig env) -join "`n"
if ($zigEnv -notmatch '\.lib_dir = "([^"]+)"') { throw 'zig env has no lib_dir' }
$zigLib = $Matches[1] -replace '\\\\', '\'
Copy-License (Join-Path (Split-Path -Parent $zigLib) 'LICENSE') 'zig-LICENSE.txt'

$Zip = Join-Path $Dist "$Name.zip"
if (Test-Path -LiteralPath $Zip) { Remove-Item -LiteralPath $Zip }
Compress-Archive -Path $App -DestinationPath $Zip -CompressionLevel Optimal
$hash = (Get-FileHash -LiteralPath $Zip -Algorithm SHA256).Hash.ToLowerInvariant()
Set-Content -LiteralPath "$Zip.sha256" -Value "$hash  $Name.zip" -Encoding ascii -NoNewline
Write-Host "Packed $Zip"
