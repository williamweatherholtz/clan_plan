-- Add optional URL slug to reunions for friendly short links (/r/:slug)
ALTER TABLE reunions ADD COLUMN slug TEXT UNIQUE;
