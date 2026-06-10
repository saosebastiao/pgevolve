-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.nodes (
  id bigint NOT NULL,
  parent_id bigint,
  CONSTRAINT nodes_pkey PRIMARY KEY (id)
);
CREATE VIEW app.node_tree AS
  WITH RECURSIVE tree (id, depth) AS (
    SELECT id, 1 AS depth FROM app.nodes WHERE parent_id IS NULL
    UNION ALL
    SELECT n.id, t.depth + 1 FROM app.nodes n JOIN tree t ON n.parent_id = t.id
  )
  SELECT id, depth FROM tree;
