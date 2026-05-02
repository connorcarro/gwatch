$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoPath = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $RepoPath

$env:PATH = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")

function Get-CargoCommand {
    $Cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($Cargo) {
        return $Cargo.Source
    }

    $CargoExe = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
    if (Test-Path $CargoExe) {
        return $CargoExe
    }

    throw "Cargo was not found. Install Rust or add Cargo to PATH."
}

function Test-WindowsRustLinker {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        return
    }

    $Link = Get-Command link.exe -ErrorAction SilentlyContinue
    if ($Link -and $Link.Source -like "*\Git\usr\bin\link.exe") {
        throw @"
Rust is finding Git for Windows' link.exe instead of the Microsoft linker:
  $($Link.Source)

Install Visual Studio Build Tools with the C++ workload and Windows SDK, then run this script from a new PowerShell window.

Recommended install command:
  winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
"@
    }
}

Test-WindowsRustLinker
$CargoCommand = Get-CargoCommand

& $CargoCommand install --path .

Write-Host ""
Write-Host "Installed gwatch."
Write-Host "If this terminal still cannot find it, open a new PowerShell window or run:"
Write-Host '$env:PATH = [Environment]::GetEnvironmentVariable("Path","Machine") + ";" + [Environment]::GetEnvironmentVariable("Path","User")'
