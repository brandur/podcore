CREATE TABLE podcast_exception (
    id BIGSERIAL PRIMARY KEY,

    -- unique because we only save the last error that occurred
    podcast_id BIGINT NOT NULL UNIQUE
        REFERENCES podcast (id) ON DELETE RESTRICT,

    errors TEXT[] NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL
);
COMMENT ON TABLE podcast_exception
    IS 'Stores exceptions that occurred when ingesting an existing podcast.';
