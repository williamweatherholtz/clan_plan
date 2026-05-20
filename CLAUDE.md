# Clan Plan — Agent Instructions

## NEVER modify a committed migration

Files under `migrations/` are an **append-only ledger**. Once a migration file
is in `main`, treat it as **byte-for-byte immutable**.

**This includes:**

- No content changes (typo fixes, schema tweaks, comment edits — all forbidden)
- No formatting changes (whitespace, indentation, trailing newlines)
- No line-ending normalization (`dos2unix`, `tr -d '\r'`, `git add --renormalize`)
- No reordering, renaming, or "cleaning up"

If you need to change the schema or seed data, **create a new migration** with
the next sequence number (e.g. `018_*.sql`).

### Why

`sqlx` records a SHA-384 of every migration when it first applies the file. On
every subsequent app startup it re-hashes the file on disk and refuses to start
if the bytes differ — even invisible changes break this. Recovering means
either patching `_sqlx_migrations.checksum` row-by-row in every database that
has already applied the migration, or wiping the volume. Both are bad. The
rule prevents the situation entirely.

### Enforcement

Three layers, in order of precedence:

1. **Persistent memory** — agents working on this project carry a feedback
   memory titled "Migrations are immutable once committed."
2. **`.gitattributes`** pins every `*.sql` file to `eol=lf` so working-tree
   line endings stay LF on every platform. Don't remove or weaken those rules.
3. **Pre-commit hook** at `scripts/git-hooks/pre-commit` (installed into
   `.git/hooks/pre-commit`) blocks any commit that modifies an existing
   migration. If it fires, the answer is to revert and write a new migration.
   Never use `git commit --no-verify` to bypass this.

### First-time setup for new clones

```sh
ln -sf ../../scripts/git-hooks/pre-commit .git/hooks/pre-commit
# or on Windows (PowerShell):
copy scripts\git-hooks\pre-commit .git\hooks\pre-commit
```

## Other project notes

- Stack: Rust + Axum + Askama (compile-time templates) + sqlx (no `query!`
  macros yet, so `SQLX_OFFLINE=true` in the Dockerfile is harmless without a
  `.sqlx/` directory).
- Frontend: Tailwind + Alpine.js + a custom Heirloom Modern design system in
  `assets/app.css` (Fraunces / Geist / JetBrains Mono).
- Run `cargo check` before committing — Askama compiles templates at build
  time, so any template error is caught here.
- `docker compose up` for local dev (Postgres + Mailpit + app).
