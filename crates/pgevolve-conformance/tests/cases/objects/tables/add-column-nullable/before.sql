-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
  id bigint NOT NULL,
  CONSTRAINT users_pkey PRIMARY KEY (id)
);
