-- Remove the 'schedule' phase: any reunion in that state moves to 'prep_completed'.
UPDATE reunions SET phase = 'prep_completed' WHERE phase = 'schedule';

CREATE TYPE reunion_phase_new AS ENUM (
    'draft',
    'availability',
    'locations',
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
