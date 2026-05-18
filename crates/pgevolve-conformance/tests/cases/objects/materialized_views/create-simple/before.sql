-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.events (
  id bigint NOT NULL,
  user_id bigint NOT NULL,
  CONSTRAINT events_pkey PRIMARY KEY (id)
);
