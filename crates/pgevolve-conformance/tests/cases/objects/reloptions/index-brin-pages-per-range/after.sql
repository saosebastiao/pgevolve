-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.t (id bigint);
CREATE INDEX i ON app.t USING brin (id) WITH (pages_per_range = 32);
