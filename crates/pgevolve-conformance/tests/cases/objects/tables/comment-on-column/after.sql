-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id   bigint NOT NULL,
    name text NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);

COMMENT ON COLUMN app.users.name IS 'Full display name of the user';
