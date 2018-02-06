table! {
    directories (id) {
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
    }
}

table! {
    directory_podcast_directory_search (id) {
        id -> Int8,
        directory_podcast_id -> Int8,
        directory_search_id -> Int8,
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
    episodes (id) {
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
    podcast_feed_content (id) {
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
        first_retrieved_at -> Timestamptz,
        feed_url -> Text,
        last_retrieved_at -> Timestamptz,
        podcast_id -> Int8,
    }
}

table! {
    podcasts (id) {
        id -> Int8,
        image_url -> Nullable<Text>,
        language -> Nullable<Text>,
        last_retrieved_at -> Timestamptz,
        link_url -> Nullable<Text>,
        title -> Text,
    }
}

joinable!(directory_podcast -> directories (directory_id));
joinable!(directory_podcast -> podcasts (podcast_id));
joinable!(directory_podcast_directory_search -> directory_podcast (directory_podcast_id));
joinable!(directory_podcast_directory_search -> directory_search (directory_search_id));
joinable!(directory_search -> directories (directory_id));
joinable!(episodes -> podcasts (podcast_id));
joinable!(podcast_feed_content -> podcasts (podcast_id));
joinable!(podcast_feed_locations -> podcasts (podcast_id));

allow_tables_to_appear_in_same_query!(
    directories,
    directory_podcast,
    directory_podcast_directory_search,
    directory_search,
    episodes,
    podcast_feed_content,
    podcast_feed_locations,
    podcasts,
);
