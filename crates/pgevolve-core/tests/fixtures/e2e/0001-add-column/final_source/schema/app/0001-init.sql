-- @pgevolve schema=app
CREATE SCHEMA app;

CREATE TABLE app.users (
    id           bigint      NOT NULL,
    email        text        NOT NULL,
    display_name text,
    created_at   timestamptz NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
