WITH expired AS (
    SELECT id
    FROM account
    WHERE email IS NULL
        AND last_seen_at < NOW() - $1::interval
    LIMIT $2
),
deleted_account_batch AS (
    DELETE FROM account
    WHERE id IN (
        SELECT id
        FROM expired
    )
    RETURNING id
),
deleted_account_podcast_batch AS (
    DELETE FROM account_podcast
    WHERE account_id IN (
        SELECT id
        FROM expired
    )
    RETURNING id
),
deleted_account_podcast_episode_batch AS (
    DELETE FROM account_podcast_episode
    WHERE account_podcast_id IN (
        SELECT id
        FROM deleted_account_podcast_batch
    )
    RETURNING id
),
deleted_key_batch AS (
    DELETE FROM key
    WHERE account_id IN (
        SELECT id
        FROM expired
    )
    RETURNING id
)
SELECT (
    (SELECT COUNT(*) FROM deleted_account_batch)
) AS count, (
    (SELECT COUNT(*) FROM deleted_account_podcast_batch) +
    (SELECT COUNT(*) FROM deleted_account_podcast_episode_batch) +
    (SELECT COUNT(*) FROM deleted_key_batch)
) AS count_related_objects;
