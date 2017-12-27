table! {
    episodes (id) {
        id -> Int8,
        enclosure_type -> Text,
        enclosure_url -> Text,
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
        title -> Text,
        url -> Text,
    }
}

joinable!(episodes -> podcasts (podcast_id));

allow_tables_to_appear_in_same_query!(episodes, podcasts,);
