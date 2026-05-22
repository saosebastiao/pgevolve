-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
GRANT SELECT ON TABLE app.t TO readers;
