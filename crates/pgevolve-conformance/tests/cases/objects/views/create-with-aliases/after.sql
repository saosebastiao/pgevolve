-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  email text,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
CREATE VIEW app.aliased_view (user_id, user_email) AS
  SELECT id, email FROM app.users;
