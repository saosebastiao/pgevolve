-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.base (
    id    bigint NOT NULL DEFAULT 1,
    email text,
    data  text,
    CONSTRAINT base_pkey      PRIMARY KEY (id),
    CONSTRAINT base_email_key UNIQUE (email),
    CONSTRAINT base_email_chk CHECK (email <> '')
);
CREATE INDEX ON app.base (data);
CREATE TABLE app.clone (LIKE app.base INCLUDING ALL);
