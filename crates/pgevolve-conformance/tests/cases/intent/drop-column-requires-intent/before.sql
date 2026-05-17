-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id bigint PRIMARY KEY,
    legacy_id text
);
