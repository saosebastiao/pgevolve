-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (id bigint PRIMARY KEY, payload jsonb);
