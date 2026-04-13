-- Migrate any existing 'travel' blocks to 'group', then drop the enum value.
UPDATE schedule_blocks SET block_type = 'group' WHERE block_type = 'travel';

-- PostgreSQL doesn't support removing enum values directly, so we recreate the type.
CREATE TYPE block_type_new AS ENUM ('group', 'optional', 'meal');

-- Must drop the column default before changing the type; PostgreSQL cannot
-- automatically cast the typed default expression to the new enum type.
ALTER TABLE schedule_blocks ALTER COLUMN block_type DROP DEFAULT;

ALTER TABLE schedule_blocks
    ALTER COLUMN block_type TYPE block_type_new
    USING block_type::text::block_type_new;

DROP TYPE block_type;
ALTER TYPE block_type_new RENAME TO block_type;

-- Restore the default now that the column uses the renamed type.
ALTER TABLE schedule_blocks ALTER COLUMN block_type SET DEFAULT 'optional';
