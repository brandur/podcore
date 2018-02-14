ALTER TABLE episode
    DROP CONSTRAINT episode_title_check,
    DROP CONSTRAINT episode_title_check1,
    DROP CONSTRAINT episode_title_check2,
    DROP CONSTRAINT episode_title_check3,
    DROP CONSTRAINT episode_title_check4,
    DROP CONSTRAINT episode_title_check5,

    ADD CONSTRAINT episode_description_check CHECK (char_length(description) <= 20000),
    ADD CONSTRAINT episode_guid_check        CHECK (char_length(guid)        <= 200),
    ADD CONSTRAINT episode_link_url_check    CHECK (char_length(link_url)    <= 500),
    ADD CONSTRAINT episode_media_type_check  CHECK (char_length(media_type)  <= 100),
    ADD CONSTRAINT episode_media_url_check   CHECK (char_length(media_url)   <= 500),
    ADD CONSTRAINT episode_title_check       CHECK (char_length(title)       <= 200)
;
