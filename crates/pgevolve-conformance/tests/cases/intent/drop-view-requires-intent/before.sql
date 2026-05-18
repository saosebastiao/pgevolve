-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
CREATE VIEW app.old_view AS
  SELECT id FROM app.users;
