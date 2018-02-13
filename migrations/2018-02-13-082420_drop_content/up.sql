ALTER TABLE podcast_feed_content
    DROP COLUMN content;
ALTER TABLE podcast_feed_content
    ALTER COLUMN content_gzip SET NOT NULL;
