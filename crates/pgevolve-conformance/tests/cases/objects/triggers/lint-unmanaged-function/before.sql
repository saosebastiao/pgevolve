-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.widgets (
  id   bigint NOT NULL,
  name text   NOT NULL,
  CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
