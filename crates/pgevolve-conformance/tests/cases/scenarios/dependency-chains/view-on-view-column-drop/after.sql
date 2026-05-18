-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  name text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
-- v1 now selects only id — incompatible body change (column narrowing)
CREATE VIEW app.v1 AS
  SELECT id FROM app.users;
-- v2 body unchanged; will be recreated transitively
CREATE VIEW app.v2 AS
  SELECT id FROM app.v1;
