-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
GRANT ALL ON TABLE app.t TO readers;
