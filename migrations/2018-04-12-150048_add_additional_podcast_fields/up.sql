ALTER TABLE directory_podcast
    ADD COLUMN image_url TEXT
        CHECK (char_length(image_url) <= 500);

ALTER TABLE podcast
    ADD COLUMN description TEXT
        CHECK (char_length(description) <= 2000);
