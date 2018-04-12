table! {
    account (id) {
        id -> Int8,
        created_at -> Timestamptz,
        email -> Nullable<Text>,
        ephemeral -> Bool,
        last_ip -> Text,
        last_seen_at -> Timestamptz,
        mobile -> Bool,
    }
}

table! {
    account_podcast (id) {
        id -> Int8,
        account_id -> Int8,
        podcast_id -> Int8,
        subscribed_at -> Timestamptz,
        unsubscribed_at -> Nullable<Timestamptz>,
    }
}

table! {
    account_podcast_episode (id) {
        id -> Int8,
        account_podcast_id -> Int8,
        episode_id -> Int8,
        favorited -> Bool,
        listened_seconds -> Nullable<Int8>,
        played -> Bool,
        updated_at -> Timestamptz,
    }
}

table! {
    directory (id) {
        id -> Int8,
        name -> Text,
    }
}

table! {
    directory_podcast (id) {
        id -> Int8,
        directory_id -> Int8,
        feed_url -> Text,
        podcast_id -> Nullable<Int8>,
        title -> Text,
        vendor_id -> Text,
        image_url -> Nullable<Text>,
    }
}

table! {
    directory_podcast_directory_search (id) {
        id -> Int8,
        directory_podcast_id -> Int8,
        directory_search_id -> Int8,
        position -> Int4,
    }
}

table! {
    directory_podcast_exception (id) {
        id -> Int8,
        directory_podcast_id -> Int8,
        errors -> Array<Text>,
        occurred_at -> Timestamptz,
    }
}

table! {
    directory_search (id) {
        id -> Int8,
        directory_id -> Int8,
        query -> Text,
        retrieved_at -> Timestamptz,
    }
}

table! {
    episode (id) {
        id -> Int8,
        description -> Nullable<Text>,
        explicit -> Nullable<Bool>,
        guid -> Text,
        link_url -> Nullable<Text>,
        media_type -> Nullable<Text>,
        media_url -> Text,
        podcast_id -> Int8,
        published_at -> Timestamptz,
        title -> Text,
    }
}

table! {
    key (id) {
        id -> Int8,
        account_id -> Int8,
        created_at -> Timestamptz,
        expire_at -> Nullable<Timestamptz>,
        secret -> Text,
    }
}

table! {
    podcast (id) {
        id -> Int8,
        image_url -> Nullable<Text>,
        language -> Nullable<Text>,
        last_retrieved_at -> Timestamptz,
        link_url -> Nullable<Text>,
        title -> Text,
        description -> Nullable<Text>,
    }
}

table! {
    podcast_exception (id) {
        id -> Int8,
        podcast_id -> Int8,
        errors -> Array<Text>,
        occurred_at -> Timestamptz,
    }
}

table! {
    podcast_feed_content (id) {
        id -> Int8,
        podcast_id -> Int8,
        retrieved_at -> Timestamptz,
        sha256_hash -> Text,
        content_gzip -> Bytea,
    }
}

table! {
    podcast_feed_location (id) {
        id -> Int8,
        first_retrieved_at -> Timestamptz,
        feed_url -> Text,
        last_retrieved_at -> Timestamptz,
        podcast_id -> Int8,
    }
}

joinable!(account_podcast -> account (account_id));
joinable!(account_podcast -> podcast (podcast_id));
joinable!(account_podcast_episode -> account_podcast (account_podcast_id));
joinable!(account_podcast_episode -> episode (episode_id));
joinable!(directory_podcast -> directory (directory_id));
joinable!(directory_podcast -> podcast (podcast_id));
joinable!(directory_podcast_directory_search -> directory_podcast (directory_podcast_id));
joinable!(directory_podcast_directory_search -> directory_search (directory_search_id));
joinable!(directory_podcast_exception -> directory_podcast (directory_podcast_id));
joinable!(directory_search -> directory (directory_id));
joinable!(episode -> podcast (podcast_id));
joinable!(key -> account (account_id));
joinable!(podcast_exception -> podcast (podcast_id));
joinable!(podcast_feed_content -> podcast (podcast_id));
joinable!(podcast_feed_location -> podcast (podcast_id));

allow_tables_to_appear_in_same_query!(
    account,
    account_podcast,
    account_podcast_episode,
    directory,
    directory_podcast,
    directory_podcast_directory_search,
    directory_podcast_exception,
    directory_search,
    episode,
    key,
    podcast,
    podcast_exception,
    podcast_feed_content,
    podcast_feed_location,
);
