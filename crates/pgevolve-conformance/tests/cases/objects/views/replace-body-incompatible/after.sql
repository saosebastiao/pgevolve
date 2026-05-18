-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  name text,
  email text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
-- Remove email column from view — incompatible (column narrowing)
CREATE VIEW app.user_report AS
  SELECT id, name FROM app.users;
