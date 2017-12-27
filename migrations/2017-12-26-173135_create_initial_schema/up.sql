CREATE TABLE podcasts (
    id BIGSERIAL PRIMARY KEY,

    title TEXT NOT NULL
        CHECK (char_length(title) <= 200),
    url TEXT NOT NULL
        CHECK (char_length(url) <= 500)
);

CREATE TABLE episodes (
    id BIGSERIAL PRIMARY KEY,

    enclosure_type TEXT NOT NULL
        CHECK (char_length(title) <= 100),
    enclosure_url TEXT NOT NULL
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
    (title, url)
VALUES
    ('Hardcore History', 'http://example.com/hardcore-history'),
    ('Road Work', 'http://example.com/road-work'),
    ('Waking Up', 'http://example.com/waking-up');
