ALTER TABLE episode
    DROP CONSTRAINT episode_title_check,
    ADD CONSTRAINT episode_title_check CHECK (char_length(title) <= 200)
;
