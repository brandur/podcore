ALTER TABLE podcast_feed_content
    ADD COLUMN content TEXT
        CHECK (char_length(content) <= 1000000);
ALTER TABLE podcast_feed_content
    ALTER COLUMN content_gzip DROP NOT NULL;
