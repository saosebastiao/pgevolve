-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
  id bigint NOT NULL,
  name text,
  CONSTRAINT events_pkey PRIMARY KEY (id)
);
