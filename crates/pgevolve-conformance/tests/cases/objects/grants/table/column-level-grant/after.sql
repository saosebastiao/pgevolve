-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint, name text);
GRANT INSERT (name) ON TABLE app.t TO readers;
