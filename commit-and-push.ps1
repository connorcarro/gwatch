param(
    [string]$Message = "Initial gwatch TUI"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoPath = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $RepoPath

# The repo was created from a sandboxed process, so allow normal Git to use it.
git config --global --add safe.directory $RepoPath

git config user.name "Connor Carro"
git config user.email "connorcarro@users.noreply.github.com"

$RemoteUrl = "connorcarro:connorcarro/gwatch.git"
if (git remote get-url origin 2>$null) {
    git remote set-url origin $RemoteUrl
} else {
    git remote add origin $RemoteUrl
}

git branch -M main
git add .gitignore Cargo.lock Cargo.toml README.md src commit-and-push.ps1

$HasChanges = git diff --cached --quiet; $LastExit = $LASTEXITCODE
if ($LastExit -eq 1) {
    git commit -m $Message
} elseif ($LastExit -eq 0) {
    Write-Host "No staged changes to commit."
} else {
    throw "git diff --cached --quiet failed with exit code $LastExit"
}

git push -u origin main
