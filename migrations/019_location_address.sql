-- Add a structured address field to location_candidates so the ICS export
-- can put a real, map-app-parseable string in each event's LOCATION field.
-- iOS Calendar (Apple Maps) and Google Calendar both auto-detect plain
-- street-address strings and offer a tap-to-navigate hand-off.

ALTER TABLE location_candidates
    ADD COLUMN address TEXT;
