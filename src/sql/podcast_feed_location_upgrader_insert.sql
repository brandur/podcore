INSERT INTO podcast_feed_location
    (first_retrieved_at,
     feed_url,
     last_retrieved_at,
     podcast_id)
SELECT
    now(),
    regexp_replace(feed_url, '^http://', 'https://'),
    now(),
    podcast_id
FROM podcast_feed_location l1
WHERE feed_url LIKE 'http://%'
    AND NOT EXISTS (
        SELECT 1
        FROM podcast_feed_location l2
        WHERE l2.podcast_id = l1.podcast_id
            AND feed_url = regexp_replace(l1.feed_url, '^http://', 'https://')
    )
    AND substring(feed_url FROM '.*://([^/]*)') IN (
        SELECT distinct(substring(feed_url FROM '.*://([^/]*)')) AS https_host
        FROM podcast_feed_location
        WHERE feed_url LIKE 'https://%'
    );
