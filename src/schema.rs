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
    podcasts (id) {
        id -> Int8,
        feed_url -> Text,
        link_url -> Text,
        title -> Text,
    }
}

joinable!(episodes -> podcasts (podcast_id));

allow_tables_to_appear_in_same_query!(episodes, podcasts,);
