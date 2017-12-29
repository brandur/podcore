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

CREATE INDEX directories_name
    ON directories (name);

INSERT INTO directories (name)
    VALUES ('Apple iTunes')
    ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name;

--
-- directory_searches
--

CREATE TABLE directory_searches (
    id BIGSERIAL PRIMARY KEY,
    directory_id BIGINT NOT NULL
        REFERENCES directories (id) ON DELETE RESTRICT,
    query TEXT NOT NULL
        CHECK (char_length(query) <= 100),
    retrieved_at TIMESTAMPTZ NOT NULL
);
COMMENT ON TABLE directory_searches
    IS 'Cached searches for podcasts on directories.';

--
-- podcasts
--

CREATE TABLE podcasts (
    id BIGSERIAL PRIMARY KEY,

    image_url TEXT
        CHECK (char_length(image_url) <= 500),
    language TEXT
        CHECK (char_length(language) <= 10),
    link_url TEXT
        CHECK (char_length(link_url) <= 500),
    title TEXT NOT NULL
        CHECK (char_length(title) <= 200)
);
COMMENT ON TABLE podcasts
    IS 'Podcast series. e.g. Roderick on the Line.';

--
-- podcast_feed_locations
--

CREATE TABLE podcast_feed_locations (
    id BIGSERIAL PRIMARY KEY,

    discovered_at TIMESTAMPTZ NOT NULL,
    feed_url TEXT NOT NULL
        CHECK (char_length(feed_url) <= 500),
    podcast_id BIGINT NOT NULL
        REFERENCES podcasts (id) ON DELETE RESTRICT
);
COMMENT ON TABLE podcast_feed_locations
    IS 'Historical records of podcast feed URLs.';

CREATE INDEX podcast_feed_locations_podcast_id_discovered_at
    ON podcast_feed_locations (podcast_id, discovered_at);
CREATE UNIQUE INDEX podcast_feed_locations_podcast_id_feed_url
    ON podcast_feed_locations (podcast_id, feed_url);

--
-- podcast_feed_contents
--

CREATE TABLE podcast_feed_contents (
    id BIGSERIAL PRIMARY KEY,
    content TEXT NOT NULL
        CHECK (char_length(content) <= 1000000),
    podcast_id BIGINT NOT NULL
        REFERENCES podcasts (id) ON DELETE RESTRICT,
    retrieved_at TIMESTAMPTZ NOT NULL,
    sha256_hash TEXT NOT NULL
        CHECK (char_length(sha256_hash) = 64)
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
    feed_url TEXT
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
-- directories_podcasts_directory_searches
--

CREATE TABLE directories_podcasts_directory_searches (
    id BIGSERIAL PRIMARY KEY,

    directories_podcasts_id BIGINT NOT NULL
        REFERENCES directories_podcasts (id) ON DELETE RESTRICT,
    directory_searches BIGINT NOT NULL
        REFERENCES directory_searches (id) ON DELETE RESTRICT
);
COMMENT ON TABLE directory_searches
    IS 'Join table between searches on directories and directory podcasts.';

--
-- episodes
--

CREATE TABLE episodes (
    id BIGSERIAL PRIMARY KEY,

    description TEXT
        CHECK (char_length(title) <= 2000),
    explicit BOOL,
    guid TEXT NOT NULL
        CHECK (char_length(title) <= 100),
    link_url TEXT
        CHECK (char_length(title) <= 500),
    media_type TEXT
        CHECK (char_length(title) <= 100),
    media_url TEXT NOT NULL
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
    (title, image_url, language, link_url)
VALUES
    ('Hardcore History', 'http://example.com/hardcore-history', 'en-US', ''),
    ('Road Work', 'http://example.com/road-work', 'en-US', ''),
    ('Waking Up', 'http://example.com/waking-up', 'en-US', '');
