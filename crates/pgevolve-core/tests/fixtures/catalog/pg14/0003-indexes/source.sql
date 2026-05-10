CREATE SCHEMA app;

CREATE TABLE app.users (
    id          bigint PRIMARY KEY,
    email       text NOT NULL,
    name        text,
    deleted_at  timestamp with time zone
);

CREATE INDEX users_email_idx ON app.users (email);
CREATE UNIQUE INDEX users_email_uniq ON app.users (email);
CREATE INDEX users_email_lower_idx ON app.users (lower(email));
CREATE INDEX users_active_idx ON app.users (email) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX users_email_with_name ON app.users (email) INCLUDE (name);
