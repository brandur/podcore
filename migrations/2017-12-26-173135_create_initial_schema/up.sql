CREATE TABLE podcasts (
    id BIGSERIAL PRIMARY KEY,

    feed_url TEXT NOT NULL
        CHECK (char_length(feed_url) <= 500),
    image_url TEXT NOT NULL
        CHECK (char_length(image_url) <= 500),
    language TEXT NOT NULL
        CHECK (char_length(language) <= 10),
    link_url TEXT NOT NULL
        CHECK (char_length(title) <= 500),
    title TEXT NOT NULL
        CHECK (char_length(title) <= 200)
);

CREATE TABLE episodes (
    id BIGSERIAL PRIMARY KEY,

    description TEXT NOT NULL
        CHECK (char_length(title) <= 2000),
    explicit BOOL NOT NULL,
    media_type TEXT NOT NULL
        CHECK (char_length(title) <= 100),
    media_url TEXT NOT NULL
        CHECK (char_length(title) <= 500),
    guid TEXT NOT NULL
        CHECK (char_length(title) <= 100),
    link_url TEXT NOT NULL
        CHECK (char_length(title) <= 500),
    podcast_id BIGINT NOT NULL
        REFERENCES podcasts (id) ON DELETE RESTRICT,
    published_at TIMESTAMPTZ NOT NULL,
    title TEXT NOT NULL
        CHECK (char_length(title) <= 200)
);

CREATE UNIQUE INDEX episodes_podcast_id_guid
    ON episodes (podcast_id, guid);

--

INSERT INTO podcasts
    (title, feed_url, image_url, language, link_url)
VALUES
    ('Hardcore History', '', 'http://example.com/hardcore-history', 'en-US', ''),
    ('Road Work', '', 'http://example.com/road-work', 'en-US', ''),
    ('Waking Up', '', 'http://example.com/waking-up', 'en-US', '');
