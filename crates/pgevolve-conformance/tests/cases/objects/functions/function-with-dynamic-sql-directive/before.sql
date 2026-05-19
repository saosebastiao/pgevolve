-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.reports (
  id bigint NOT NULL,
  name text,
  CONSTRAINT reports_pkey PRIMARY KEY (id)
);
