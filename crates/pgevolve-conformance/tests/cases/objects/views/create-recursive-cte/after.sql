-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.nodes (
  id bigint NOT NULL,
  parent_id bigint,
  CONSTRAINT nodes_pkey PRIMARY KEY (id)
);
CREATE RECURSIVE VIEW app.node_tree (id, depth) AS
  SELECT id, 0 AS depth FROM app.nodes WHERE parent_id IS NULL
  UNION ALL
  SELECT n.id, t.depth + 1 FROM app.nodes n JOIN node_tree t ON n.parent_id = t.id;
