[CmdletBinding()]
param(
    [string]$InnoCompiler,
    [switch]$TestInstallMode
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$projectRoot = Split-Path -Parent $PSScriptRoot
$work = Join-Path $projectRoot 'artifacts/installer-build'
$cargoOutput = Join-Path $work 'cargo'
$downloads = Join-Path $work 'downloads'
$stage = Join-Path $work 'package'
$output = if ($TestInstallMode) { Join-Path $work 'qa' } else { Join-Path $projectRoot 'artifacts/installer' }
$target = 'x86_64-pc-windows-msvc'
$mihomoVersion = 'v1.19.33'
$coreHash = 'A4FBF2B8BC2B5416CE2D0D392AC59F2E0A234EA4FCF635B66BD21E62EA9641B9'
$releaseArchive = "mihomo-windows-amd64-v1-$mihomoVersion.zip"
$sourceArchive = "mihomo-$mihomoVersion-source.zip"

function Test-Sha256([string]$Path, [string]$Expected) {
    (Test-Path -LiteralPath $Path -PathType Leaf) -and
        ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -eq $Expected)
}

function Get-VerifiedDownload([string]$Url, [string]$Path, [string]$Expected) {
    if (Test-Sha256 $Path $Expected) { return }
    $partial = "$Path.partial"
    try {
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $partial -TimeoutSec 120
        if (-not (Test-Sha256 $partial $Expected)) {
            throw "SHA-256 mismatch for $Url"
        }
        Move-Item -LiteralPath $partial -Destination $Path -Force
    } finally {
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial }
    }
}

if (-not $InnoCompiler) {
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    $InnoCompiler = if ($command) {
        $command.Source
    } else {
        Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'
    }
}
if (-not (Test-Path -LiteralPath $InnoCompiler -PathType Leaf)) {
    throw 'Install Inno Setup 6.3+ or pass -InnoCompiler with the path to ISCC.exe.'
}
$InnoCompiler = (Resolve-Path -LiteralPath $InnoCompiler).Path

New-Item -ItemType Directory -Force -Path $downloads, $stage, $output | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$localCore = Join-Path $projectRoot 'vendor/mihomo/mihomo.exe'
$stagedCore = Join-Path $stage 'mihomo.exe'
if (Test-Sha256 $localCore $coreHash) {
    Copy-Item -LiteralPath $localCore -Destination $stagedCore -Force
} elseif (-not (Test-Sha256 $stagedCore $coreHash)) {
    $zipPath = Join-Path $downloads $releaseArchive
    Get-VerifiedDownload "https://github.com/MetaCubeX/mihomo/releases/download/$mihomoVersion/$releaseArchive" `
        $zipPath 'FB0B960EFF568459502D4EBE53E9CCA203053D86EC9024FE935F19615D74D9AA'
    $zip = [IO.Compression.ZipFile]::OpenRead($zipPath)
    try {
        $executables = @($zip.Entries | Where-Object { $_.Name -like 'mihomo*.exe' })
        if ($executables.Count -ne 1) { throw 'Unexpected files in the Mihomo release archive.' }
        [IO.Compression.ZipFileExtensions]::ExtractToFile($executables[0], $stagedCore, $true)
    } finally {
        $zip.Dispose()
    }
}
if (-not (Test-Sha256 $stagedCore $coreHash)) { throw 'Mihomo executable SHA-256 mismatch.' }

$sourceZip = Join-Path $stage $sourceArchive
Get-VerifiedDownload "https://codeload.github.com/MetaCubeX/mihomo/zip/refs/tags/$mihomoVersion" `
    $sourceZip 'BB705AA873FC0E9DB352FDC69B992C93FAD5B310756E0E5F81194A0E0C108E21'
$zip = [IO.Compression.ZipFile]::OpenRead($sourceZip)
try {
    $license = $zip.GetEntry("mihomo-$($mihomoVersion.TrimStart('v'))/LICENSE")
    if (-not $license) { throw 'Mihomo source archive does not contain LICENSE.' }
    [IO.Compression.ZipFileExtensions]::ExtractToFile($license, (Join-Path $stage 'Mihomo.LICENSE.txt'), $true)
} finally {
    $zip.Dispose()
}
Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination (Join-Path $stage 'Everty-MultiLink.LICENSE.txt') -Force
Copy-Item -LiteralPath (Join-Path $projectRoot 'assets/meta-rules-dat.LICENSE') -Destination (Join-Path $stage 'Meta-Rules-Dat.LICENSE.txt') -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Third-party.txt') -Destination $stage -Force

$rustFlags = 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS'
$previousRustFlags = [Environment]::GetEnvironmentVariable($rustFlags, 'Process')
Push-Location -LiteralPath $projectRoot
try {
    [Environment]::SetEnvironmentVariable($rustFlags, '-C target-feature=+crt-static', 'Process')
    $metadata = cargo metadata --no-deps --locked --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $clientPackage = $metadata.packages | Where-Object name -eq 'multilink-app'
    $version = $clientPackage.version
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'Expected a numeric release version in multilink-app/Cargo.toml.' }
    $messages = cargo build --release --locked --target $target --target-dir $cargoOutput -p multilink-app --message-format=json-render-diagnostics
    if ($LASTEXITCODE -ne 0) { throw 'Client release build failed.' }
} finally {
    [Environment]::SetEnvironmentVariable($rustFlags, $previousRustFlags, 'Process')
    Pop-Location
}

$releaseDir = Join-Path $cargoOutput "$target/release"
Copy-Item -LiteralPath (Join-Path $releaseDir 'multilink-app.exe') -Destination $stage -Force
$clientBuildScript = $messages | ForEach-Object { ConvertFrom-Json -InputObject $_ } |
    Where-Object { $_.reason -eq 'build-script-executed' -and $_.package_id -eq $clientPackage.id } |
    Select-Object -Last 1
if (-not $clientBuildScript) { throw 'Cargo did not report the client build script output.' }
$icon = Join-Path $clientBuildScript.out_dir 'app.ico'
if (-not (Test-Path -LiteralPath $icon -PathType Leaf)) { throw 'The client build did not produce the installer icon.' }
Copy-Item -LiteralPath $icon -Destination $stage -Force

$name = "EvertyMultiLink-$version-windows-x64-setup"
$compilerArgs = @("/O$output", "/F$name", "/DAppVersion=$version", "/DStageDir=$stage")
if ($TestInstallMode) { $compilerArgs += '/DTestInstallMode' }
& $InnoCompiler @compilerArgs (Join-Path $PSScriptRoot 'setup.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }

$installer = Join-Path $output "$name.exe"
$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $name.exe" | Set-Content -LiteralPath "$installer.sha256" -Encoding ascii
Write-Output $installer

$portableName = "EvertyMultiLink-$version-windows-x64-portable"
$portableZip = Join-Path $output "$portableName.zip"
if (Test-Path -LiteralPath $portableZip) { Remove-Item -LiteralPath $portableZip -Force }
Compress-Archive -Path "$stage\*" -DestinationPath $portableZip -Force
$zipHash = (Get-FileHash -LiteralPath $portableZip -Algorithm SHA256).Hash.ToLowerInvariant()
"$zipHash  $portableName.zip" | Set-Content -LiteralPath "$portableZip.sha256" -Encoding ascii
Write-Output $portableZip
