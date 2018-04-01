--
-- account
--

CREATE TABLE account (
    id BIGSERIAL PRIMARY KEY,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    email TEXT,
    ephemeral BOOLEAN NOT NULL,

    last_ip TEXT NOT NULL
        CHECK (char_length(last_ip) <= 100),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CHECK (
        (ephemeral AND email IS NULL)
        OR
        (NOT ephemeral AND email IS NOT NULL)
    )
);

CREATE INDEX account_email
    ON account (email) WHERE email IS NOT NULL;

-- Used for cleaning ephemeral accounts.
CREATE INDEX account_last_seen_at
    ON account (last_seen_at)
    WHERE email IS NULL;

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

    favorite BOOLEAN NOT NULL DEFAULT false,

    -- Play progress, or the second in playtime to which the user has listened.
    listened_seconds BIGINT CHECK (listened_seconds >= 0),

    played BOOLEAN NOT NULL DEFAULT false,
    updated_at TIMESTAMPTZ NOT NULL,

    -- An episode is either played fully with a `NULL` `listened_seconds`, or it
    -- has a `listened_seconds` value and is not `played`.
    CHECK (
        (played AND listened_seconds IS NULL)
        OR
        (NOT played AND listened_seconds IS NOT NULL)
    )
);

CREATE UNIQUE INDEX account_podcast_episode_account_podcast_id_episode_id
    ON account_podcast_episode (account_podcast_id, episode_id);

---
--- key
---

CREATE TABLE key (
    id BIGSERIAL PRIMARY KEY,

    account_id BIGINT NOT NULL
        REFERENCES account (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expire_at TIMESTAMPTZ,
    secret TEXT NOT NULL
        CHECK (char_length(secret) = 60)
);

CREATE INDEX key_account_id
    ON key (account_id);

-- Used for cleaning expired keys.
CREATE INDEX key_expire_at
    ON key (expire_at)
    WHERE expire_at IS NOT NULL;

CREATE INDEX key_secret
    ON key (secret);
