-- RA-configurable assumed duration for activities that have no explicit end time.
ALTER TABLE reunions
    ADD COLUMN default_activity_duration_minutes INTEGER NOT NULL DEFAULT 60;
