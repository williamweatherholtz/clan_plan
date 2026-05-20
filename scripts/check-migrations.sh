#!/usr/bin/env bash
# Validate the migrations/ directory so a broken state can't ship in a docker
# image or sneak into git. Two classes of check:
#
#   1. Sequence:  filenames are NNN_*.sql, contiguous from 001, no gaps,
#                 no duplicates. Catches the "DB has migration N, file is
#                 missing" failure mode we hit at deploy time.
#   2. Git:       (only when run inside a working tree) no untracked,
#                 deleted-but-unstaged, or unstaged-deleted migrations.
#                 Catches "the file is on my disk but won't be in the image"
#                 and "I rm'd it but forgot to commit".
#
# Skip the git half with SKIP_GIT_CHECK=1 (used by the Dockerfile, since the
# build context doesn't include .git).
set -euo pipefail

MIGRATIONS_DIR="${MIGRATIONS_DIR:-migrations}"

if [[ ! -d "$MIGRATIONS_DIR" ]]; then
    echo "check-migrations: missing directory: $MIGRATIONS_DIR" >&2
    exit 1
fi

shopt -s nullglob
files=("$MIGRATIONS_DIR"/[0-9][0-9][0-9]_*.sql)
shopt -u nullglob

# Also flag any *.sql that didn't match the NNN_*.sql shape.
mapfile -t all_sql < <(find "$MIGRATIONS_DIR" -maxdepth 1 -name '*.sql' -printf '%f\n' | sort)
for name in "${all_sql[@]}"; do
    if [[ ! "$name" =~ ^[0-9]{3}_.+\.sql$ ]]; then
        echo "check-migrations: malformed migration filename: $MIGRATIONS_DIR/$name" >&2
        echo "  expected pattern: NNN_description.sql (e.g. 018_add_foo.sql)" >&2
        exit 1
    fi
done

if [[ ${#files[@]} -eq 0 ]]; then
    echo "check-migrations: no migration files found in $MIGRATIONS_DIR" >&2
    exit 1
fi

# Sort by filename — leading-zero numbering means lexical = numeric.
IFS=$'\n' files=($(printf '%s\n' "${files[@]}" | sort))
unset IFS

expected=1
for path in "${files[@]}"; do
    name="$(basename "$path")"
    num="${name:0:3}"
    # strip leading zeros for arithmetic (10# forces base-10)
    n=$((10#$num))
    if (( n != expected )); then
        printf 'check-migrations: sequence gap — expected %03d, found %s\n' "$expected" "$name" >&2
        echo "  every migration version must be present; a gap means a previously-applied" >&2
        echo "  migration is missing from disk and sqlx will refuse to boot." >&2
        exit 1
    fi
    expected=$((expected + 1))
done

last=$((expected - 1))
printf 'check-migrations: OK — %d migrations, 001..%03d contiguous\n' "${#files[@]}" "$last"

# ── Git-tree consistency (skipped inside docker build) ────────────────────────
if [[ "${SKIP_GIT_CHECK:-0}" == "1" ]]; then
    exit 0
fi

if ! command -v git >/dev/null 2>&1; then
    exit 0
fi

if ! git rev-parse --git-dir >/dev/null 2>&1; then
    exit 0
fi

fail=0

untracked="$(git ls-files --others --exclude-standard -- "$MIGRATIONS_DIR" 2>/dev/null || true)"
if [[ -n "$untracked" ]]; then
    echo "check-migrations: untracked migration files (won't be in any image):" >&2
    echo "$untracked" | sed 's/^/  /' >&2
    fail=1
fi

deleted="$(git ls-files --deleted -- "$MIGRATIONS_DIR" 2>/dev/null || true)"
if [[ -n "$deleted" ]]; then
    echo "check-migrations: migration files deleted from working tree but not committed:" >&2
    echo "$deleted" | sed 's/^/  /' >&2
    echo "  this is the exact shape of the 'previously applied but missing' failure." >&2
    fail=1
fi

if (( fail )); then
    exit 1
fi

# ── Lockfile checksum check ───────────────────────────────────────────────────
# migrations/.lock pins the sha256 of every committed migration file. The
# pre-commit hook already blocks edits to committed migrations; this script
# catches the case where the hook didn't run (fresh clone, --no-verify, CI
# checkout) by failing the build/deploy if any byte has drifted.
#
# Regenerate after intentionally adding a new migration:
#   scripts/check-migrations.sh --update-lock
LOCKFILE="$MIGRATIONS_DIR/.lock"

# Choose a portable sha256 invoker.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

if [[ "${1:-}" == "--update-lock" ]]; then
    : > "$LOCKFILE"
    for path in "${files[@]}"; do
        name="$(basename "$path")"
        sum="$(sha256_of "$path")"
        printf '%s  %s\n' "$sum" "$name" >> "$LOCKFILE"
    done
    echo "check-migrations: wrote $LOCKFILE (${#files[@]} entries)"
    exit 0
fi

if [[ ! -f "$LOCKFILE" ]]; then
    # First run: surface the situation but don't fail. The next intentional
    # change will trigger --update-lock and the lock takes effect from then on.
    echo "check-migrations: no $LOCKFILE — skipping checksum check (run with --update-lock to create)" >&2
    exit 0
fi

lock_fail=0
declare -A lock_map=()
while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" =~ ^# ]] && continue
    sum="${line%% *}"
    file="${line##* }"
    lock_map["$file"]="$sum"
done < "$LOCKFILE"

for path in "${files[@]}"; do
    name="$(basename "$path")"
    expected="${lock_map[$name]:-}"
    actual="$(sha256_of "$path")"
    if [[ -z "$expected" ]]; then
        echo "check-migrations: $name has no entry in $LOCKFILE — run --update-lock to add" >&2
        lock_fail=1
        continue
    fi
    if [[ "$expected" != "$actual" ]]; then
        echo "check-migrations: checksum mismatch for $name" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $actual" >&2
        echo "  if this change is intentional, run scripts/check-migrations.sh --update-lock" >&2
        echo "  if not, restore the file (typical cause: line-ending drift on Windows)." >&2
        lock_fail=1
    fi
    unset 'lock_map[$name]'
done

# Anything left in lock_map was committed previously and is now missing from disk.
for stale in "${!lock_map[@]}"; do
    echo "check-migrations: $stale is in $LOCKFILE but missing from disk" >&2
    lock_fail=1
done

if (( lock_fail )); then
    exit 1
fi

printf 'check-migrations: lockfile OK (%d entries match)\n' "${#files[@]}"
