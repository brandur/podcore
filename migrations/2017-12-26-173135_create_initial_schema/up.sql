CREATE TABLE podcasts (
    id BIGSERIAL PRIMARY KEY,
    title TEXT NOT NULL
        CHECK (char_length(title) <= 100),
    url TEXT NOT NULL
        CHECK (char_length(url) <= 500)
);

INSERT INTO podcasts
    (title, url)
VALUES
    ('Hardcore History', 'http://example.com/hardcore-history'),
    ('Road Work', 'http://example.com/road-work'),
    ('Waking Up', 'http://example.com/waking-up');
