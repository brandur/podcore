--
-- directory
--

CREATE TABLE directory (
    id BIGSERIAL PRIMARY KEY,

    name TEXT NOT NULL UNIQUE
        CHECK (char_length(name) <= 100)
);
COMMENT ON TABLE directory
    IS 'Podcast directory. e.g. Apple iTunes.';

CREATE INDEX directory_name
    ON directory (name);

INSERT INTO directory (name)
    VALUES ('Apple iTunes')
    ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name;

--
-- directory_search
--

CREATE TABLE directory_search (
    id BIGSERIAL PRIMARY KEY,

    directory_id BIGINT NOT NULL
        REFERENCES directory (id) ON DELETE RESTRICT,
    query TEXT NOT NULL
        CHECK (char_length(query) <= 100),
    retrieved_at TIMESTAMPTZ NOT NULL
);
COMMENT ON TABLE directory_search
    IS 'Cached searches for podcast on directory.';

--
-- podcast
--

CREATE TABLE podcast (
    id BIGSERIAL PRIMARY KEY,

    image_url TEXT
        CHECK (char_length(image_url) <= 500),
    language TEXT
        CHECK (char_length(language) <= 10),
    last_retrieved_at TIMESTAMPTZ NOT NULL,
    link_url TEXT
        CHECK (char_length(link_url) <= 500),
    title TEXT NOT NULL
        CHECK (char_length(title) <= 200)
);
COMMENT ON TABLE podcast
    IS 'Podcast series. e.g. Roderick on the Line.';

--
-- podcast_feed_location
--

CREATE TABLE podcast_feed_location (
    id BIGSERIAL PRIMARY KEY,

    first_retrieved_at TIMESTAMPTZ NOT NULL,
    feed_url TEXT NOT NULL
        CHECK (char_length(feed_url) <= 500),
    last_retrieved_at TIMESTAMPTZ NOT NULL,
    podcast_id BIGINT NOT NULL
        REFERENCES podcast (id) ON DELETE RESTRICT
);
COMMENT ON TABLE podcast_feed_location
    IS 'Historical records of podcast feed URLs.';

CREATE INDEX podcast_feed_location_podcast_id_first_retrieved_at
    ON podcast_feed_location (podcast_id, first_retrieved_at);
CREATE UNIQUE INDEX podcast_feed_location_podcast_id_feed_url
    ON podcast_feed_location (podcast_id, feed_url);

--
-- podcast_feed_content
--

CREATE TABLE podcast_feed_content (
    id BIGSERIAL PRIMARY KEY,

    content TEXT NOT NULL
        CHECK (char_length(content) <= 1000000),
    podcast_id BIGINT NOT NULL
        REFERENCES podcast (id) ON DELETE RESTRICT,
    retrieved_at TIMESTAMPTZ NOT NULL,
    sha256_hash TEXT NOT NULL
        CHECK (char_length(sha256_hash) = 64)
);
COMMENT ON TABLE podcast_feed_content
    IS 'Historical records of raw content retrieved from podcast feeds.';
COMMENT ON COLUMN podcast_feed_content.content
    IS 'Raw XML content.';

CREATE INDEX podcast_feed_content_podcast_id_retrieved_at
    ON podcast_feed_content (podcast_id, retrieved_at);
CREATE UNIQUE INDEX podcast_feed_content_podcast_id_sha256_hash
    ON podcast_feed_content (podcast_id, sha256_hash);

--
-- directory_podcast
--

CREATE TABLE directory_podcast (
    id BIGSERIAL PRIMARY KEY,

    directory_id BIGINT NOT NULL
        REFERENCES directory (id) ON DELETE RESTRICT,
    feed_url TEXT NOT NULL
        CHECK (char_length(feed_url) <= 500),
    podcast_id BIGINT
        REFERENCES podcast (id) ON DELETE RESTRICT,
    title TEXT NOT NULL
        CHECK (char_length(title) <= 500),
    vendor_id TEXT NOT NULL
        CHECK (char_length(vendor_id) <= 200)
);
COMMENT ON TABLE directory_podcast
    IS 'Podcast series. e.g. Roderick on the Line.';
COMMENT ON COLUMN directory_podcast.feed_url
    IS 'Podcast''s feed URL. Useful when retrieving a podcast''s feed for the first time.';
COMMENT ON COLUMN directory_podcast.podcast_id
    IS 'Internal podcast ID. Only assigned after the podcast''s feed is retrieved for the first time.';
COMMENT ON COLUMN directory_podcast.title
    IS 'Podcast''s title.';
COMMENT ON COLUMN directory_podcast.vendor_id
    IS 'A unique ID for the podcast which is assigned by the directory''s vendor.';

CREATE UNIQUE INDEX directory_podcast_directory_id_vendor_id
    ON directory_podcast (directory_id, vendor_id);

--
-- directory_podcast_directory_search
--

CREATE TABLE directory_podcast_directory_search (
    id BIGSERIAL PRIMARY KEY,

    directory_podcast_id BIGINT NOT NULL
        REFERENCES directory_podcast (id) ON DELETE RESTRICT,

    -- ON DELETE CASCADE because I'm too lazy to issue multiple operations when
    -- I'm cleaning these up and because this all recoverable ephemeral data
    -- anyway
    directory_search_id BIGINT NOT NULL
        REFERENCES directory_search (id) ON DELETE CASCADE
);
COMMENT ON TABLE directory_podcast_directory_search
    IS 'Join table between searches on directory and directory podcast.';

CREATE INDEX directory_podcast_directory_search_directory_podcast_id
    ON directory_podcast_directory_search (directory_podcast_id);

CREATE INDEX directory_podcast_directory_search_directory_search_id
    ON directory_podcast_directory_search (directory_search_id);

--
-- directory_podcast_exception
--

CREATE TABLE directory_podcast_exception (
    id BIGSERIAL PRIMARY KEY,

    -- unique because we only save the last error that occurred
    directory_podcast_id BIGINT NOT NULL UNIQUE
        REFERENCES directory_podcast (id) ON DELETE RESTRICT,

    errors TEXT[] NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL
);
COMMENT ON TABLE directory_podcast_exception
    IS 'Stores exceptions that occurred when ingesting a podcast.';

--
-- episode
--

CREATE TABLE episode (
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
        REFERENCES podcast (id) ON DELETE RESTRICT,
    published_at TIMESTAMPTZ NOT NULL,
    title TEXT NOT NULL
        CHECK (char_length(title) <= 200)
);
COMMENT ON TABLE episode
    IS 'Podcast episode, like a single item in a podcast''s RSS feed.';

CREATE UNIQUE INDEX episode_podcast_id_guid
    ON episode (podcast_id, guid);
