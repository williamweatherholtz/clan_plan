-- Add category to activity ideas: group, optional (default), meal
ALTER TABLE activity_ideas
    ADD COLUMN category TEXT NOT NULL DEFAULT 'optional'
    CONSTRAINT activity_category_valid
        CHECK (category IN ('group', 'optional', 'meal'));
