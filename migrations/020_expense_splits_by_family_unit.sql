-- Shift the granularity of shared-expense splits from individual users to
-- family units. A reunion's "shared" costs (groceries, gas, lodging, etc.)
-- are assumed to be consumed equally by each participating family — a unit
-- of 5 and a unit of 1 each owe the same share.
--
-- sqlx wraps each migration in a single transaction, so all of the
-- statements below either land together or not at all.

ALTER TABLE expense_splits
    ADD COLUMN family_unit_id UUID REFERENCES family_units(id) ON DELETE CASCADE;

UPDATE expense_splits es
SET family_unit_id = u.family_unit_id
FROM users u
WHERE u.id = es.user_id;

-- Drop rows whose user has no family unit assigned — they can't be mapped
-- under the new model. (Rare; users without a unit usually shouldn't show
-- up on a split list to begin with.)
DELETE FROM expense_splits WHERE family_unit_id IS NULL;

-- Consolidate duplicates: under the old per-user model, multiple users from
-- the same family unit may have appeared on the same split. Sum their
-- amounts so each (expense, unit) gets exactly one row.
CREATE TEMP TABLE _consolidated_splits ON COMMIT DROP AS
SELECT expense_id, family_unit_id, SUM(amount_cents)::INTEGER AS amount_cents
FROM expense_splits
GROUP BY expense_id, family_unit_id;

TRUNCATE expense_splits;

ALTER TABLE expense_splits DROP COLUMN user_id;
ALTER TABLE expense_splits ALTER COLUMN family_unit_id SET NOT NULL;
ALTER TABLE expense_splits ADD CONSTRAINT expense_splits_unit_uniq
    UNIQUE (expense_id, family_unit_id);

INSERT INTO expense_splits (expense_id, family_unit_id, amount_cents)
SELECT expense_id, family_unit_id, amount_cents FROM _consolidated_splits;
