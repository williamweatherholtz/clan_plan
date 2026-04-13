-- Allow the RA to set an explicit date range for the availability poll.
-- When set, only these dates are shown in the availability calendar.
-- When NULL, falls back to the confirmed reunion dates (or 90-day window).
ALTER TABLE reunions ADD COLUMN avail_poll_start DATE;
ALTER TABLE reunions ADD COLUMN avail_poll_end   DATE;
