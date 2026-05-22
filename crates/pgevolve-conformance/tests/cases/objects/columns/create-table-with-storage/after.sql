-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.docs (
    id   bigint PRIMARY KEY,
    body text STORAGE EXTERNAL COMPRESSION lz4
);
