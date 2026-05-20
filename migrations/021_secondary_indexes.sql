-- Secondary indexes flagged by the architecture critique (P3-38, P3-39):
--
-- 1. activity_rsvps PK is (activity_idea_id, user_id, role). Lookups by
--    user_id alone (e.g. "show me everything this user RSVPed to") can't
--    use the PK index because user_id isn't the leading column, so Postgres
--    falls back to a seq scan.
--
-- 2. expense_splits got a UNIQUE(expense_id, family_unit_id) in migration
--    020 which serves balances_for_reunion fine, but any future "what does
--    family unit X owe across all expenses" query has no usable index
--    starting from family_unit_id.

CREATE INDEX IF NOT EXISTS idx_activity_rsvps_user
    ON activity_rsvps (user_id);

CREATE INDEX IF NOT EXISTS idx_expense_splits_family_unit
    ON expense_splits (family_unit_id);
