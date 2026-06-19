param(
    [string]$InstallDir = "$env:LOCALAPPDATA\RemoText"
)

$ErrorActionPreference = "Stop"
$Repo = "Rorical/RemoText"
$BinName = "remotext.exe"

function Write-Info  { Write-Host $args -ForegroundColor Cyan }
function Write-Ok    { Write-Host $args -ForegroundColor Green }
function Write-Err   { Write-Host $args -ForegroundColor Red }

function Get-Platform {
    $arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else { "x86" }
    return "windows-$arch"
}

function Get-LatestVersion {
    $url = "https://api.github.com/repos/$Repo/releases/latest"
    $response = Invoke-RestMethod -Uri $url -Headers @{
        Accept = "application/vnd.github+json"
        "X-GitHub-Api-Version" = "2022-11-28"
    }
    return $response.tag_name
}

function Install-RemoText {
    param([string]$Platform, [string]$Version)

    $asset = "remotext-$Platform.zip"
    $url = "https://github.com/$Repo/releases/download/$Version/$asset"

    Write-Info "Downloading RemoText $Version for $Platform..."
    $tmpdir = Join-Path $env:TEMP "remotext-install-$(Get-Random)"
    New-Item -ItemType Directory -Force $tmpdir | Out-Null

    try {
        $zipPath = Join-Path $tmpdir $asset
        Invoke-WebRequest -Uri $url -OutFile $zipPath

        Write-Info "Extracting..."
        Expand-Archive -Path $zipPath -DestinationPath $tmpdir -Force

        New-Item -ItemType Directory -Force $InstallDir | Out-Null
        $exePath = Join-Path $tmpdir $BinName
        if (-not (Test-Path $exePath)) {
            $exePath = Join-Path $tmpdir "package\$BinName"
        }
        if (-not (Test-Path $exePath)) {
            throw "Could not find $BinName in extracted archive"
        }

        Copy-Item -Path $exePath -Destination (Join-Path $InstallDir $BinName) -Force
        Write-Ok "Installed RemoText $Version to $InstallDir\$BinName"

        $currentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
        if ($currentPath -notlike "*$InstallDir*") {
            [Environment]::SetEnvironmentVariable(
                "PATH", "$currentPath;$InstallDir", "User"
            )
            $env:PATH = "$env:PATH;$InstallDir"
            Write-Info "Added $InstallDir to user PATH."
        }
    }
    finally {
        Remove-Item -Recurse -Force $tmpdir -ErrorAction SilentlyContinue
    }
}

function Main {
    Write-Info "RemoText one-click installer"

    $platform = Get-Platform
    Write-Info "Detected platform: $platform"

    $version = Get-LatestVersion
    Write-Info "Latest version: $version"

    Install-RemoText -Platform $platform -Version $version

    Write-Info "Run 'remotext --help' to get started."
}

Main
