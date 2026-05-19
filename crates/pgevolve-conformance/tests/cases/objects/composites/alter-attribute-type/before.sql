-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.measurement AS (value integer, unit text);
