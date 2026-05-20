#!/usr/bin/env pwsh
# Install the project's git hooks for the current clone. Idempotent.
#
# The hook in scripts/git-hooks/ blocks edits to committed migration files
# (see CLAUDE.md "NEVER modify a committed migration"). Per-clone install
# is needed because git hooks aren't versioned automatically. Windows path
# uses copy (not symlink) since symlinks under .git/hooks/ are unreliable
# without admin / dev-mode enabled.

$ErrorActionPreference = 'Stop'

$repoRoot = git rev-parse --show-toplevel
if (-not $?) {
    Write-Error "install-hooks: not in a git repo"
    exit 1
}
Set-Location $repoRoot

if (-not (Test-Path .git)) {
    Write-Error "install-hooks: .git directory not found at $repoRoot"
    exit 1
}

if (-not (Test-Path .git/hooks)) {
    New-Item -ItemType Directory -Path .git/hooks | Out-Null
}

Copy-Item scripts/git-hooks/pre-commit .git/hooks/pre-commit -Force
Write-Host "install-hooks: installed scripts/git-hooks/pre-commit -> .git/hooks/pre-commit"
