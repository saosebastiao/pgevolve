-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.base (id bigint);
CREATE MATERIALIZED VIEW app.m WITH (fillfactor = 90) AS SELECT id FROM app.base;
