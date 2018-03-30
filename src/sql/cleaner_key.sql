WITH expired AS (
    SELECT id
    FROM key
    WHERE expire_at IS NOT NULL
        AND expire_at < NOW() - $1::interval
    LIMIT $2
),
deleted_batch AS (
    DELETE FROM key
    WHERE id IN (
        SELECT id
        FROM expired
    )
    RETURNING id
)
SELECT COUNT(*)
FROM deleted_batch;
