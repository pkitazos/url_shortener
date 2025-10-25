CREATE TABLE url (
    long_url varchar not null,
    short_code varchar not null
);

CREATE UNIQUE INDEX url_short_index on url (short_code);
CREATE UNIQUE INDEX url_long_index on url (long_url);
