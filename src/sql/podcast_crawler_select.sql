--
-- Differentiate podcasts that have seen some kind of update even remotely
-- recently and those that haven't. This allows us to back off to a much --
-- more conservative crawl cadence for podcasts that almost never see updates
-- (and in some cases, may never see an update again).
--
WITH podcast_with_refresh_interval AS (
    SELECT podcast.id,
        podcast.last_retrieved_at,
        CASE
            WHEN podcast_feed_content.retrieved_at > NOW() - '1 month'::interval
                THEN $1::interval
            ELSE $2::interval
        END AS refresh_interval
    FROM podcast
        INNER JOIN podcast_feed_content
            ON podcast.id = podcast_feed_content.podcast_id
)
SELECT id,
    (
       SELECT feed_url
       FROM podcast_feed_location
       WHERE podcast_feed_location.podcast_id = podcast_with_refresh_interval.id
       ORDER BY last_retrieved_at DESC
       LIMIT 1
    )
FROM podcast_with_refresh_interval
WHERE id > $3
    AND last_retrieved_at - trunc(random() * $4) * '1 minute'::interval
        <= NOW() - refresh_interval
ORDER BY id
LIMIT $5;
