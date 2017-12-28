table! {
    directories (id) {
        id -> Int8,
        name -> Text,
    }
}

table! {
    directories_podcasts (id) {
        id -> Int8,
        directory_id -> Int8,
        feed_url -> Nullable<Text>,
        podcast_id -> Nullable<Int8>,
        vendor_id -> Text,
    }
}

table! {
    directories_podcasts_directory_searches (id) {
        id -> Int8,
        directories_podcasts_id -> Int8,
        directory_searches -> Int8,
    }
}

table! {
    directory_searches (id) {
        id -> Int8,
        directory_id -> Int8,
        query -> Text,
        retrieved_at -> Timestamptz,
    }
}

table! {
    episodes (id) {
        id -> Int8,
        description -> Text,
        explicit -> Bool,
        media_type -> Text,
        media_url -> Text,
        guid -> Text,
        link_url -> Text,
        podcast_id -> Int8,
        published_at -> Timestamptz,
        title -> Text,
    }
}

table! {
    podcast_feed_contents (id) {
        id -> Int8,
        content -> Text,
        podcast_id -> Int8,
        retrieved_at -> Timestamptz,
        sha256_hash -> Text,
    }
}

table! {
    podcast_feed_locations (id) {
        id -> Int8,
        discovered_at -> Timestamptz,
        feed_url -> Text,
        podcast_id -> Int8,
    }
}

table! {
    podcasts (id) {
        id -> Int8,
        image_url -> Text,
        language -> Text,
        link_url -> Text,
        title -> Text,
    }
}

joinable!(directories_podcasts -> directories (directory_id));
joinable!(directories_podcasts -> podcasts (podcast_id));
joinable!(directories_podcasts_directory_searches -> directories_podcasts (directories_podcasts_id));
joinable!(directories_podcasts_directory_searches -> directory_searches (directory_searches));
joinable!(directory_searches -> directories (directory_id));
joinable!(episodes -> podcasts (podcast_id));
joinable!(podcast_feed_contents -> podcasts (podcast_id));
joinable!(podcast_feed_locations -> podcasts (podcast_id));

allow_tables_to_appear_in_same_query!(
    directories,
    directories_podcasts,
    directories_podcasts_directory_searches,
    directory_searches,
    episodes,
    podcast_feed_contents,
    podcast_feed_locations,
    podcasts,
);
