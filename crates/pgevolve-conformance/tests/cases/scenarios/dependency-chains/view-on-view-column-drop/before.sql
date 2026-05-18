-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  name text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
-- v1 selects two columns from users
CREATE VIEW app.v1 AS
  SELECT id, name FROM app.users;
-- v2 selects from v1
CREATE VIEW app.v2 AS
  SELECT id FROM app.v1;
