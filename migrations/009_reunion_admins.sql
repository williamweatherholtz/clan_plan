CREATE TABLE reunion_admins (
    reunion_id UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    added_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (reunion_id, user_id)
);
CREATE INDEX idx_reunion_admins_reunion ON reunion_admins(reunion_id);

-- Migrate existing single-RA assignments
INSERT INTO reunion_admins (reunion_id, user_id)
SELECT id, responsible_admin_id FROM reunions
WHERE responsible_admin_id IS NOT NULL;

-- Drop the old column
ALTER TABLE reunions DROP COLUMN responsible_admin_id;
