#!/usr/bin/env bash
# Install the project's git hooks for the current clone. Idempotent.
#
# The hook in scripts/git-hooks/ blocks edits to committed migration files
# (see CLAUDE.md "NEVER modify a committed migration"). Per-clone install
# is needed because git hooks aren't versioned automatically.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT"

if [[ ! -d .git ]]; then
    echo "install-hooks: not in a git repo ($REPO_ROOT)" >&2
    exit 1
fi

mkdir -p .git/hooks
cp scripts/git-hooks/pre-commit .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit

echo "install-hooks: installed scripts/git-hooks/pre-commit → .git/hooks/pre-commit"
