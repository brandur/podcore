WITH expired AS (
    SELECT id
    FROM directory_search
    WHERE retrieved_at < NOW() - $1::interval
    LIMIT $2
),
deleted_batch AS (
    DELETE FROM directory_search
    WHERE id IN (
        SELECT id
        FROM expired
    )
    RETURNING id
)
SELECT COUNT(*)
FROM deleted_batch;
