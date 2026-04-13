-- Add PrepCompleted phase (between Schedule and Active) and timezone to locations.

-- ── Location timezone ─────────────────────────────────────────────────────────
-- Existing rows get UTC; new rows must supply a real IANA timezone string.
ALTER TABLE location_candidates ADD COLUMN timezone TEXT NOT NULL DEFAULT 'UTC';

-- ── reunion_phase enum — add prep_completed ───────────────────────────────────
CREATE TYPE reunion_phase_new AS ENUM (
    'draft',
    'availability',
    'locations',
    'schedule',
    'prep_completed',
    'active',
    'post_reunion',
    'archived'
);

ALTER TABLE reunions ALTER COLUMN phase DROP DEFAULT;

ALTER TABLE reunions
    ALTER COLUMN phase TYPE reunion_phase_new
    USING phase::text::reunion_phase_new;

DROP TYPE reunion_phase;
ALTER TYPE reunion_phase_new RENAME TO reunion_phase;

ALTER TABLE reunions ALTER COLUMN phase SET DEFAULT 'draft';
