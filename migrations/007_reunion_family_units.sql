-- Track which family units are participating in a given reunion.
-- The RA selects these via the Settings page; progress bars and
-- weighted voting are computed relative to this set.

CREATE TABLE reunion_family_units (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reunion_id      UUID NOT NULL REFERENCES reunions(id) ON DELETE CASCADE,
    family_unit_id  UUID NOT NULL REFERENCES family_units(id) ON DELETE CASCADE,
    added_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(reunion_id, family_unit_id)
);

CREATE INDEX idx_rfu_reunion ON reunion_family_units (reunion_id);
