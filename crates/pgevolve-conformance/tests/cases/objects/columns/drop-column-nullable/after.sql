-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id    bigint NOT NULL,
    email text,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
