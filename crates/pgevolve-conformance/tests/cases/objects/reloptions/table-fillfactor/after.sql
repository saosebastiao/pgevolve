-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint) WITH (fillfactor = 80);
