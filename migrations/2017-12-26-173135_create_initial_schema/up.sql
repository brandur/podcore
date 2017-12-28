--
-- directories
--

CREATE TABLE directories (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
        CHECK (char_length(name) <= 100)
);
COMMENT ON TABLE directories
    IS 'Podcast directories. e.g. Apple iTunes.';

INSERT INTO directories (name)
    VALUES ('Apple iTunes')
    ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name;

--
-- podcasts
--

CREATE TABLE podcasts (
    id BIGSERIAL PRIMARY KEY,

    feed_url TEXT NOT NULL
        CHECK (char_length(feed_url) <= 500),
    image_url TEXT NOT NULL
        CHECK (char_length(image_url) <= 500),
    language TEXT NOT NULL
        CHECK (char_length(language) <= 10),
    link_url TEXT NOT NULL
        CHECK (char_length(link_url) <= 500),
    title TEXT NOT NULL
        CHECK (char_length(title) <= 200)
);
COMMENT ON TABLE podcasts
    IS 'Podcast series. e.g. Roderick on the Line.';

--
-- podcast_feed_contents
--

CREATE TABLE podcast_feed_contents (
    id BIGSERIAL PRIMARY KEY,
    content TEXT NOT NULL
        CHECK (char_length(content) <= 100000),
    podcast_id BIGINT NOT NULL
        REFERENCES podcasts (id) ON DELETE RESTRICT,
    retrieved_at TIMESTAMPTZ NOT NULL
);
COMMENT ON TABLE podcast_feed_contents
    IS 'Historical records of raw content retrieved from podcast feeds.';
COMMENT ON COLUMN podcast_feed_contents.content
    IS 'Raw XML content.';

CREATE INDEX podcast_feed_contents_podcast_id_retrieved_at
    ON podcast_feed_contents (podcast_id, retrieved_at);

--
-- directories_podcasts
--

CREATE TABLE directories_podcasts (
    id BIGSERIAL PRIMARY KEY,
    directory_id BIGINT NOT NULL
        REFERENCES directories (id) ON DELETE RESTRICT,
    feed_url TEXT NOT NULL
        CHECK (char_length(feed_url) <= 500),
    podcast_id BIGINT
        REFERENCES podcasts (id) ON DELETE RESTRICT,
    vendor_id TEXT NOT NULL
        CHECK (char_length(vendor_id) <= 200)
);
COMMENT ON TABLE directories_podcasts
    IS 'Podcast series. e.g. Roderick on the Line.';
COMMENT ON COLUMN directories_podcasts.feed_url
    IS 'Podcast''s feed URL. Useful when retrieving a podcast''s feed for the first time and unset after.';
COMMENT ON COLUMN directories_podcasts.podcast_id
    IS 'Internal podcast ID. Only assigned after the podcast''s feed is retrieved for the first time.';
COMMENT ON COLUMN directories_podcasts.vendor_id
    IS 'A unique ID for the podcast which is assigned by the directory''s vendor.';

CREATE UNIQUE INDEX directories_podcasts_directory_id_vendor_id
    ON directories_podcasts (directory_id, vendor_id);

--
-- episodes
--

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
COMMENT ON TABLE episodes
    IS 'Podcast episodes, like a single item in a podcast''s RSS feed.';

CREATE UNIQUE INDEX episodes_podcast_id_guid
    ON episodes (podcast_id, guid);

--
-- sample data
--

INSERT INTO podcasts
    (title, feed_url, image_url, language, link_url)
VALUES
    ('Hardcore History', '', 'http://example.com/hardcore-history', 'en-US', ''),
    ('Road Work', '', 'http://example.com/road-work', 'en-US', ''),
    ('Waking Up', '', 'http://example.com/waking-up', 'en-US', '');
