--
-- account
--


CREATE TABLE account (
    id BIGSERIAL PRIMARY KEY,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    email TEXT UNIQUE,
    ephemeral BOOLEAN NOT NULL,

    last_ip TEXT NOT NULL
        CHECK (char_length(last_ip) <= 100),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Indicates that this account was created for a mobile user.
    mobile BOOLEAN NOT NULL,

    password_scrypt TEXT
        CHECK (char_length(last_ip) <= 200),
    verified BOOLEAN,

    CHECK (
        (ephemeral AND verified IS NULL)
        OR
        (NOT ephemeral AND verified IS NOT NULL)
    ),

    CHECK (
        (ephemeral AND email IS NULL)
        OR
        (NOT ephemeral AND email IS NOT NULL)
    ),

    CHECK (
        (ephemeral AND password_scrypt IS NULL)
        OR
        (NOT ephemeral AND password_scrypt IS NOT NULL)
    )
);

CREATE UNIQUE INDEX account_email
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

    subscribed_at TIMESTAMPTZ,

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

    favorited BOOLEAN NOT NULL DEFAULT false,

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

-- An index that can be used to check the most recent episodes that an account
-- has played, favorited, or updated.
CREATE INDEX account_podcast_episode_account_podcast_id_updated_at
    ON account_podcast_episode (account_podcast_id, updated_at);

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

--
-- verification_code
--

CREATE TABLE verification_code (
    id BIGSERIAL PRIMARY KEY,

    account_id BIGINT NOT NULL
        REFERENCES account (id) ON DELETE RESTRICT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    secret TEXT NOT NULL UNIQUE
        CHECK (char_length(secret) <= 100)
);

CREATE INDEX verification_code_account_id
    ON verification_code (account_id);
CREATE INDEX verification_code_secret
    ON verification_code (secret);
