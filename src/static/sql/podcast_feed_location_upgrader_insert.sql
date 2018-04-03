WITH podcast_feed_location_with_host AS (
    SELECT
        feed_url,
        substring(feed_url FROM '.*://([^/]*)') AS host,
        podcast_id
    FROM podcast_feed_location
)
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
FROM podcast_feed_location_with_host l1
WHERE feed_url LIKE 'http://%'
    AND NOT EXISTS (
        SELECT 1
        FROM podcast_feed_location_with_host l2
        WHERE l2.podcast_id = l1.podcast_id
            AND feed_url = regexp_replace(l1.feed_url, '^http://', 'https://')
    )
    AND (
        host IN (
            SELECT distinct(host)
            FROM podcast_feed_location_with_host
            WHERE feed_url LIKE 'https://%'
        )

        -- We don't consider subdomains the same host for purposes of TLS
        -- upgrades because a service could have a certificate provisioned for
        -- a parent domain, but not its children. However, we do know that some
        -- services can offer TLS for everything, so they're whitelisted
        -- specially here.
        OR host LIKE '%.libsyn.com'
    );
