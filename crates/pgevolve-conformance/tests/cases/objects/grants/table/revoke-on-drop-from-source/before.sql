-- @pgevolve schema=app
CREATE SCHEMA app;
GRANT USAGE ON SCHEMA app TO readers;
CREATE TABLE app.t (id bigint);
GRANT SELECT ON TABLE app.t TO readers;
