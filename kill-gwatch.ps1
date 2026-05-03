param(
    [switch]$WhatIf
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path -LiteralPath $PSScriptRoot).Path
$currentPid = $PID

function Test-GwatchProcess {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Process
    )

    $name = [string]$Process.Name
    $exe = [string]$Process.ExecutablePath
    $commandLine = [string]$Process.CommandLine

    if ($Process.ProcessId -eq $currentPid) {
        return $false
    }

    if ($name -ieq "gwatch.exe" -or $name -ieq "gwatch") {
        return $true
    }

    if ($exe -and $exe.StartsWith($repoRoot, [System.StringComparison]::OrdinalIgnoreCase) -and
        ($exe.EndsWith("\gwatch.exe", [System.StringComparison]::OrdinalIgnoreCase) -or
         $exe.EndsWith("/gwatch.exe", [System.StringComparison]::OrdinalIgnoreCase))) {
        return $true
    }

    if ($commandLine -and
        $commandLine.IndexOf($repoRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
        ($commandLine -match "(^|\s)cargo(\.exe)?\s+run(\s|$)" -or
         $commandLine.IndexOf("gwatch", [System.StringComparison]::OrdinalIgnoreCase) -ge 0)) {
        return $true
    }

    return $false
}

$matchesById = @{}

Get-Process | ForEach-Object {
    $candidate = [pscustomobject]@{
        ProcessId = $_.Id
        Name = $_.ProcessName
        ExecutablePath = $null
        CommandLine = $null
    }

    try {
        $candidate.ExecutablePath = $_.Path
    } catch {
        $candidate.ExecutablePath = $null
    }

    if (Test-GwatchProcess -Process $candidate) {
        $matchesById[$candidate.ProcessId] = $candidate
    }
}

try {
    Get-CimInstance Win32_Process -ErrorAction Stop | ForEach-Object {
        if (Test-GwatchProcess -Process $_) {
            $matchesById[$_.ProcessId] = $_
        }
    }
} catch {
    Write-Verbose "Command-line process scan unavailable: $($_.Exception.Message)"
}

$matches = $matchesById.Values | Sort-Object ProcessId

if (-not $matches) {
    Write-Host "No gwatch processes found."
    exit 0
}

foreach ($process in $matches) {
    $label = "$($process.Name) pid=$($process.ProcessId)"

    if ($WhatIf) {
        Write-Host "Would stop $label"
        continue
    }

    Write-Host "Stopping $label"
    Stop-Process -Id $process.ProcessId -Force -ErrorAction Stop
}
