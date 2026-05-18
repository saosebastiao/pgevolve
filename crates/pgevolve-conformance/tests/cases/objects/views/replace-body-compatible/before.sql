-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  name text,
  email text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
CREATE VIEW app.active_users AS
  SELECT id, name FROM app.users;
