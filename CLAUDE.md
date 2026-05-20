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

## Writing migrations

Lessons baked in after the 018/019/020 deploys:

- **Prefer UPDATE over TRUNCATE+INSERT** when you're recreating row shapes
  in place (e.g. adding a column derived from old data, consolidating
  duplicates). TRUNCATE loses any foreign keys that point at the truncated
  table's `id` column — works today only because no FK points at
  `expense_splits.id` or similar, but the next migration that needs to
  reference one of these IDs from elsewhere will hit a silent break.

- **Migrations run inside a single sqlx transaction**, so multi-statement
  schema changes are atomic. Don't write `BEGIN`/`COMMIT` inside a
  migration file — sqlx already wraps it.

- **Don't put `ROLLBACK` in a migration**; it rolls back the migration
  itself and sqlx records the version as failed. There is no recovery
  short of `_sqlx_migrations` row surgery.

- **Always update `migrations/.lock`** after adding/renumbering a file:
  `bash scripts/check-migrations.sh --update-lock`.

## Recovery: migration-checksum mismatch in production

If sqlx refuses to boot with `migration N was previously applied but has been
modified`, the file on disk no longer matches the SHA-384 stored in
`_sqlx_migrations.checksum` for that version. Typical causes: line-ending
drift on Windows, intentional edit to a committed file (which should never
happen — see above), or two branches each adding their own migration N
followed by a merge.

Recovery steps (in order of preference):

1. **Always start with diagnosis.** Exec into the DB container and dump
   `_sqlx_migrations` plus the on-disk file to confirm what diverged:

   ```sh
   docker exec -i <db-container> psql -U clanplan -d clanplan \
     -c "SELECT version, description, encode(checksum,'hex') FROM _sqlx_migrations ORDER BY version;"
   ```

2. **If the file is unchanged but the checksum is from a CRLF version** (the
   common case after the `.gitattributes` normalization landed), recompute
   the LF SHA-384 locally and `UPDATE _sqlx_migrations SET checksum = ...
   WHERE version = N;`. This is what `scripts/fix-migration-checksums.sql`
   does for migrations 1-17 in bulk.

3. **If a renumber happened** (two branches both used the same N, one was
   manually applied to prod first), the safe fix is the same UPDATE plus an
   INSERT for the new version with `success=true` and the correct checksum,
   IFF the schema state already matches. Verify with `\d <touched_table>`
   first.

4. **NEVER edit the migration file to "match" the deployed checksum.** That
   propagates the divergence and makes every other env diverge from yours.
   Patch the DB row, not the file.

5. After recovery, `bash scripts/check-migrations.sh --update-lock` so
   `migrations/.lock` reflects the new state and CI / future hooks catch
   any subsequent drift.

## Other project notes

- Stack: Rust + Axum + Askama (compile-time templates) + sqlx (no `query!`
  macros yet, so `SQLX_OFFLINE=true` in the Dockerfile is harmless without a
  `.sqlx/` directory).
- Frontend: Tailwind + Alpine.js + a custom Heirloom Modern design system in
  `assets/app.css` (Fraunces / Geist / JetBrains Mono).
- Run `cargo check` before committing — Askama compiles templates at build
  time, so any template error is caught here.
- `docker compose up` for local dev (Postgres + Mailpit + app).
