-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  name text,
  email text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
-- Add email column to view — compatible (appended at end)
CREATE VIEW app.active_users AS
  SELECT id, name, email FROM app.users;
