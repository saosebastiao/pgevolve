-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.status AS ENUM ('open', 'closed');
CREATE TABLE app.events (
  id bigint NOT NULL,
  current_status app.status NOT NULL,
  CONSTRAINT events_pkey PRIMARY KEY (id)
);
