-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text
);
-- Column-level STORAGE in CREATE TABLE was added in PG 16; the ALTER
-- form works on every supported PG version, so we use it here to keep
-- the fixture's `min` at 14.
ALTER TABLE app.docs ALTER COLUMN body SET STORAGE EXTERNAL;
