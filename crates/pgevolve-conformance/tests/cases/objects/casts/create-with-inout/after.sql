-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE DOMAIN app.label AS text;
CREATE DOMAIN app.tag AS text;
CREATE CAST (app.label AS app.tag) WITH INOUT;
