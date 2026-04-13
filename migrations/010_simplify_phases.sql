-- Remove the intermediate "confirmed" states: date_selected and location_selected.
-- Reunions stuck in a removed phase are nudged forward to the next real phase.
UPDATE reunions SET phase = 'locations'  WHERE phase = 'date_selected';
UPDATE reunions SET phase = 'schedule'   WHERE phase = 'location_selected';

CREATE TYPE reunion_phase_new AS ENUM (
    'draft',
    'availability',
    'locations',
    'schedule',
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

-- Restore the default (still valid in the new enum).
ALTER TABLE reunions ALTER COLUMN phase SET DEFAULT 'draft';
