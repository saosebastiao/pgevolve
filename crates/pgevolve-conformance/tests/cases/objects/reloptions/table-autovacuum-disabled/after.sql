-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint) WITH (autovacuum_enabled = false);
