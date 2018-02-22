WITH deleted_batch AS (
    DELETE FROM directory_podcast
    WHERE id IN (
        SELECT id
        FROM directory_podcast
        WHERE podcast_id IS NULL
            AND NOT EXISTS (
                SELECT 1
                FROM directory_podcast_directory_search
                WHERE directory_podcast_id = directory_podcast.id
            )
            AND NOT EXISTS (
                SELECT 1
                FROM directory_podcast_exception
                WHERE directory_podcast_id = directory_podcast.id
            )
        LIMIT $1
    )
    RETURNING id
)
SELECT COUNT(*)
FROM deleted_batch;
