ALTER TABLE directory_podcast_directory_search
    ADD COLUMN position INT
        CHECK (position >= 0 AND position < 1000);

CREATE UNIQUE INDEX directory_podcast_directory_search_directory_search_id_position
    ON directory_podcast_directory_search (directory_search_id, position);
