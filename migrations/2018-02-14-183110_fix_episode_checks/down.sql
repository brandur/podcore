ALTER TABLE episode
    DROP CONSTRAINT episode_description_check,
    DROP CONSTRAINT episode_guid_check,
    DROP CONSTRAINT episode_link_url_check,
    DROP CONSTRAINT episode_media_type_check,
    DROP CONSTRAINT episode_media_url_check,
    DROP CONSTRAINT episode_title_check,

    ADD CONSTRAINT episode_title_check  CHECK (char_length(title) <= 2000),
    ADD CONSTRAINT episode_title_check1 CHECK (char_length(title) <= 100),
    ADD CONSTRAINT episode_title_check2 CHECK (char_length(title) <= 500),
    ADD CONSTRAINT episode_title_check3 CHECK (char_length(title) <= 100),
    ADD CONSTRAINT episode_title_check4 CHECK (char_length(title) <= 500),
    ADD CONSTRAINT episode_title_check5 CHECK (char_length(title) <= 200)
;
