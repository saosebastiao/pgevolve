-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.jobs (
  id bigint NOT NULL,
  status text,
  CONSTRAINT jobs_pkey PRIMARY KEY (id)
);
