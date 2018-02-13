ALTER TABLE podcast_feed_content
    ADD COLUMN content_gzip BYTEA
        CHECK (length(content_gzip) <= 1000000);
