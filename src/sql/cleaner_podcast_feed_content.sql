WITH numbered AS (
    SELECT id, podcast_id,
        ROW_NUMBER() OVER (
            PARTITION BY podcast_id
            ORDER BY retrieved_at DESC
        )
    FROM podcast_feed_content
),
excess AS (
    SELECT id, podcast_id, row_number
    FROM numbered
    WHERE row_number > $1
    LIMIT $2
),
deleted_batch AS (
    DELETE FROM podcast_feed_content
    WHERE id IN (
        SELECT id
        FROM excess
    )
    RETURNING id
)
SELECT COUNT(*)
FROM deleted_batch;
