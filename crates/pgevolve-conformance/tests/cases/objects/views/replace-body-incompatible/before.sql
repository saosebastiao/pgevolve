-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  name text,
  email text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
CREATE VIEW app.user_report AS
  SELECT id, name, email FROM app.users;
