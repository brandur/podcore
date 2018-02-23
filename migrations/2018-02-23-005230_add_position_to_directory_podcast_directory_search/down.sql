DROP INDEX directory_podcast_directory_search_directory_search_id_position;

ALTER TABLE directory_podcast_directory_search
    DROP COLUMN position;
