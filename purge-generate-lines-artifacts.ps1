param(
    [string]$Remote = "origin",
    [switch]$ForcePush,
    [switch]$DeleteLocalCopies,
    [switch]$Yes
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoPath = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $RepoPath

$ArtifactPaths = @(
    "generate_lines.exe",
    "generate_lines.pdb",
    "generate_lines",
    "tests/scripts/generate_lines.exe",
    "tests/scripts/generate_lines.pdb",
    "tests/scripts/generate_lines",
    "output.txt"
)

function Invoke-Git {
    param(
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Arguments
    )

    & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Confirm-DestructiveAction {
    param(
        [string]$Message
    )

    if ($Yes) {
        return
    }

    Write-Host ""
    Write-Host $Message
    $Answer = Read-Host "Type PURGE to continue"
    if ($Answer -cne "PURGE") {
        throw "Aborted."
    }
}

function Get-FilterRepoCommand {
    $GitFilterRepo = Get-Command git-filter-repo -ErrorAction SilentlyContinue
    if ($GitFilterRepo) {
        return @($GitFilterRepo.Source)
    }

    & git filter-repo --version *> $null
    if ($LASTEXITCODE -eq 0) {
        return @("git", "filter-repo")
    }

    throw @"
git-filter-repo is required for this cleanup.

Install it, then rerun this script:
  python -m pip install git-filter-repo

If Python scripts are not on PATH after installing, open a new PowerShell window and try again.
"@
}

function Get-RemoteUrls {
    param(
        [string]$Name
    )

    $FetchUrl = & git remote get-url $Name 2> $null
    if ($LASTEXITCODE -ne 0) {
        return $null
    }

    $PushUrl = & git remote get-url --push $Name 2> $null
    if ($LASTEXITCODE -ne 0 -or -not $PushUrl) {
        $PushUrl = $FetchUrl
    }

    return [pscustomobject]@{
        Name = $Name
        FetchUrl = [string]$FetchUrl
        PushUrl = [string]$PushUrl
    }
}

function Restore-RemoteUrls {
    param(
        [Parameter(Mandatory = $true)]
        [object]$RemoteInfo
    )

    $ExistingRemotes = @(& git remote)
    if ($LASTEXITCODE -ne 0) {
        throw "git remote failed with exit code $LASTEXITCODE"
    }

    if ($ExistingRemotes -contains $RemoteInfo.Name) {
        Invoke-Git remote set-url $RemoteInfo.Name $RemoteInfo.FetchUrl
    } else {
        Invoke-Git remote add $RemoteInfo.Name $RemoteInfo.FetchUrl
    }

    if ($RemoteInfo.PushUrl -ne $RemoteInfo.FetchUrl) {
        Invoke-Git remote set-url --push $RemoteInfo.Name $RemoteInfo.PushUrl
    }
}

function Invoke-FilterRepo {
    param(
        [string[]]$Paths
    )

    $Command = @(Get-FilterRepoCommand)
    $Args = @("--force")
    foreach ($Path in $Paths) {
        $Args += @("--path", $Path)
    }
    $Args += "--invert-paths"

    if ($Command.Length -eq 1) {
        & $Command[0] @Args
    } else {
        & $Command[0] $Command[1] @Args
    }

    if ($LASTEXITCODE -ne 0) {
        throw "git-filter-repo failed with exit code $LASTEXITCODE"
    }
}

function Remove-LocalArtifactCopies {
    param(
        [string[]]$Paths
    )

    foreach ($Path in $Paths) {
        if (Test-Path -LiteralPath $Path) {
            Write-Host "Deleting local file: $Path"
            Remove-Item -LiteralPath $Path -Force
        }
    }
}

function Test-ArtifactHistory {
    param(
        [string[]]$Paths
    )

    $Objects = & git rev-list --objects --all
    if ($LASTEXITCODE -ne 0) {
        throw "git rev-list --objects --all failed with exit code $LASTEXITCODE"
    }

    $Pattern = ($Paths | ForEach-Object { [regex]::Escape($_) }) -join "|"
    $Matches = @($Objects | Select-String -Pattern $Pattern)
    return $Matches
}

Invoke-Git rev-parse --is-inside-work-tree | Out-Null

$RemoteInfo = Get-RemoteUrls -Name $Remote
if ($ForcePush -and -not $RemoteInfo) {
    throw "Remote '$Remote' was not found."
}

$Status = & git status --porcelain
if ($LASTEXITCODE -ne 0) {
    throw "git status --porcelain failed with exit code $LASTEXITCODE"
}

if ($Status) {
    Write-Host "Your working tree is not clean. Commit or stash current cleanup changes before rewriting history."
    Write-Host ""
    $Status | ForEach-Object { Write-Host $_ }
    throw "Aborted to avoid mixing uncommitted work with a history rewrite."
}

Write-Host "This will rewrite local git history and remove these paths from every commit:"
$ArtifactPaths | ForEach-Object { Write-Host "  $_" }

Confirm-DestructiveAction "History rewriting is destructive for collaborators. Everyone must re-clone or hard-reset after the force push."

Invoke-FilterRepo -Paths $ArtifactPaths

if ($RemoteInfo) {
    Restore-RemoteUrls -RemoteInfo $RemoteInfo
}

Write-Host "Expiring reflogs and pruning local unreachable objects..."
Invoke-Git reflog expire --expire=now --expire-unreachable=now --all
Invoke-Git gc --prune=now --aggressive

if ($DeleteLocalCopies) {
    Remove-LocalArtifactCopies -Paths $ArtifactPaths
}

$Remaining = @(Test-ArtifactHistory -Paths $ArtifactPaths)
if ($Remaining.Count -gt 0) {
    Write-Host ""
    Write-Host "These artifact paths still appear in reachable local history:"
    $Remaining | ForEach-Object { Write-Host $_.Line }
    throw "Cleanup verification failed."
}

Write-Host ""
Write-Host "Local reachable history no longer contains the artifact paths."

if ($ForcePush) {
    Confirm-DestructiveAction "About to force-push every rewritten branch and tag to '$Remote'."

    Write-Host "Force-pushing branches..."
    Invoke-Git push --force --all $Remote

    Write-Host "Force-pushing tags..."
    Invoke-Git push --force --tags $Remote

    Write-Host ""
    Write-Host "Remote refs were force-pushed."
} else {
    Write-Host "Remote was not changed. Rerun with -ForcePush after reviewing the rewritten history."
}

Write-Host ""
Write-Host "Important: this cannot erase copies already fetched by other people, forks, backups, or external caches."
Write-Host "If the repository was public, rotate anything sensitive that may have been exposed."
