-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.nodes (
  id bigint NOT NULL,
  parent_id bigint,
  CONSTRAINT nodes_pkey PRIMARY KEY (id)
);
