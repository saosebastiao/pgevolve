-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE EXTENSION IF NOT EXISTS btree_gin;
CREATE TABLE app.t (id bigint);
CREATE INDEX i ON app.t USING gin (id) WITH (fastupdate = false);
