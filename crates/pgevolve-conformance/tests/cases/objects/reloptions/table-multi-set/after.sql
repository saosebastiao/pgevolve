-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80, autovacuum_enabled = false, parallel_workers = 4);
