--
-- job
--

CREATE TABLE job (
    id BIGSERIAL PRIMARY KEY,
    args JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- This is not going to be immediately used for anything, but is designed
    -- to be a control rod that allows us to insert jobs that are not to be
    -- worked.
    live BOOLEAN NOT NULL DEFAULT false,

    name TEXT NOT NULL
        CHECK (char_length(name) <= 100),
    num_errors INT NOT NULL DEFAULT 0,
    try_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX job_try_at
    ON job (try_at) WHERE live = true;
