-- Allow multiple responses per user per question (drop the unique constraint).
-- Users may now add as many comments as they like for a given question.
ALTER TABLE survey_responses
    DROP CONSTRAINT survey_responses_survey_question_id_user_id_key;
