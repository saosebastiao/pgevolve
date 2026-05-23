-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
CREATE INDEX i ON app.t (id) WITH (fillfactor = 70);
