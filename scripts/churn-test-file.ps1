param(
    [string]$Path = "test.md",
    [int]$DelaySeconds = 1
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoPath = Split-Path -Parent $PSScriptRoot
Set-Location $RepoPath

$Random = [System.Random]::new()
$Counter = 1

if (-not (Test-Path $Path)) {
    Set-Content -Path $Path -Value "# gwatch test file"
}

Write-Host "Editing $Path every $DelaySeconds second(s). Press Ctrl+C to stop."

while ($true) {
    $Lines = @(Get-Content -Path $Path -ErrorAction SilentlyContinue)

    $CanRemove = $Lines.Count -gt 1
    $ShouldAdd = (-not $CanRemove) -or ($Random.Next(0, 2) -eq 0)

    if ($ShouldAdd) {
        $Timestamp = Get-Date -Format "HH:mm:ss"
        $Line = "- random change $Counter at $Timestamp"
        Add-Content -Path $Path -Value $Line
        Write-Host "added:   $Line"
        $Counter++
    } else {
        $Index = $Random.Next(1, $Lines.Count)
        $Removed = $Lines[$Index]
        $NextLines = @()
        for ($i = 0; $i -lt $Lines.Count; $i++) {
            if ($i -ne $Index) {
                $NextLines += $Lines[$i]
            }
        }
        Set-Content -Path $Path -Value $NextLines
        Write-Host "removed: $Removed"
    }

    Start-Sleep -Seconds $DelaySeconds
}
