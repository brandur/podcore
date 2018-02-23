-- For ease, just remove all cached search queries as they might interfere with
-- this operation.
TRUNCATE TABLE directory_search CASCADE;

ALTER TABLE directory_podcast_directory_search
    ALTER COLUMN position SET NOT NULL;
