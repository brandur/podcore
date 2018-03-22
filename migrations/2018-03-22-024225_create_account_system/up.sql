--
-- account
--

CREATE TABLE account (
    id BIGSERIAL PRIMARY KEY,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    last_ip TEXT NOT NULL
        CHECK (char_length(last_ip) <= 100),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

--
-- account_podcast
--

CREATE TABLE account_podcast (
    id BIGSERIAL PRIMARY KEY,

    account_id BIGINT NOT NULL
        REFERENCES account (id) ON DELETE RESTRICT,
    podcast_id BIGINT NOT NULL
        REFERENCES podcast (id) ON DELETE RESTRICT,

    subscribed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- This is the equivalent of removing a subscription. We never delete these
    -- records so that in case a user resubscribes, they get to keep all the
    -- records of which episodes they've listened to.
    unsubscribed_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX account_podcast_account_id_podcast_id
    ON account_podcast (account_id, podcast_id);
--
-- account_podcast_episode
--

CREATE TABLE account_podcast_episode (
    id BIGSERIAL PRIMARY KEY,

    account_podcast_id BIGINT NOT NULL
        REFERENCES account_podcast (id) ON DELETE RESTRICT,
    episode_id BIGINT NOT NULL
        REFERENCES episode (id) ON DELETE RESTRICT,

    -- Play progress, or the second in playtime to which the user has listened.
    listened_second BIGINT DEFAULT 0,

    played BOOLEAN DEFAULT false
);

CREATE UNIQUE INDEX account_podcast_episode_account_podcast_id_episode_id
    ON account_podcast_episode (account_podcast_id, episode_id);
