param(
    [string]$Version = "latest",
    [string]$InstallDir = "",
    [string]$BaseUrl = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-NormalizedVersion {
    param([string]$InputVersion)
    if ($InputVersion -eq "latest") {
        return "latest"
    }
    if ($InputVersion.StartsWith("v")) {
        return $InputVersion
    }
    return "v$InputVersion"
}

function Get-DefaultInstallDir {
    if (-not [string]::IsNullOrWhiteSpace($InstallDir)) {
        return $InstallDir
    }
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        throw "LOCALAPPDATA is required to determine the default install directory."
    }
    return (Join-Path $env:LOCALAPPDATA "Programs\yacli\bin")
}

function Get-AssetName {
    $arch = $env:PROCESSOR_ARCHITECTURE
    switch -Regex ($arch) {
        "^(AMD64|x86_64)$" { return "yacli-x86_64-pc-windows-msvc.zip" }
        default { throw "Unsupported Windows architecture for yacli install: $arch" }
    }
}

function Get-DownloadBaseUrl {
    if (-not [string]::IsNullOrWhiteSpace($BaseUrl)) {
        return $BaseUrl.TrimEnd("/")
    }
    $repo = "NextStat/yacli"
    $normalized = Get-NormalizedVersion -InputVersion $Version
    if ($normalized -eq "latest") {
        return "https://github.com/$repo/releases/latest/download"
    }
    return "https://github.com/$repo/releases/download/$normalized"
}

$resolvedInstallDir = Get-DefaultInstallDir
$assetName = Get-AssetName
$downloadBaseUrl = Get-DownloadBaseUrl

$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("yacli-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
    $assetPath = Join-Path $tempDir $assetName
    $checksumsPath = Join-Path $tempDir "SHA256SUMS"
    Invoke-WebRequest -Uri "$downloadBaseUrl/$assetName" -OutFile $assetPath
    Invoke-WebRequest -Uri "$downloadBaseUrl/SHA256SUMS" -OutFile $checksumsPath

    $expectedHash = $null
    foreach ($line in Get-Content -Path $checksumsPath) {
        $parts = $line -split "\s+", 2
        if ($parts.Length -eq 2 -and $parts[1].Trim() -eq $assetName) {
            $expectedHash = $parts[0].Trim().ToLowerInvariant()
            break
        }
    }
    if ([string]::IsNullOrWhiteSpace($expectedHash)) {
        throw "Could not find checksum for $assetName in SHA256SUMS"
    }

    $actualHash = (Get-FileHash -Path $assetPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "Checksum mismatch for $assetName"
    }

    $extractDir = Join-Path $tempDir "extract"
    Expand-Archive -Path $assetPath -DestinationPath $extractDir -Force
    $binaryPath = Get-ChildItem -Path $extractDir -Filter "yacli.exe" -Recurse | Select-Object -First 1
    if ($null -eq $binaryPath) {
        throw "Could not find yacli.exe in archive $assetName"
    }

    New-Item -ItemType Directory -Path $resolvedInstallDir -Force | Out-Null
    $destination = Join-Path $resolvedInstallDir "yacli.exe"
    Copy-Item -Path $binaryPath.FullName -Destination $destination -Force

    Write-Host "Installed yacli to $destination"
    $pathEntries = ($env:PATH -split ";") | ForEach-Object { $_.Trim() }
    if ($pathEntries -notcontains $resolvedInstallDir) {
        Write-Host "Add $resolvedInstallDir to PATH to use yacli directly."
    }
}
finally {
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
