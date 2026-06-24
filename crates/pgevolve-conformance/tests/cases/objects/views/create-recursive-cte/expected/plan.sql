-- @pgevolve plan id=79fb0760561e5f56 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.node_tree
CREATE VIEW app.node_tree (id, depth) AS
WITH RECURSIVE node_tree(id, depth) AS (SELECT id, 0 AS depth FROM app.nodes WHERE parent_id IS NULL UNION ALL SELECT n.id, t.depth + 1 FROM app.nodes n JOIN node_tree t ON n.parent_id = t.id) SELECT id, depth FROM node_tree;
COMMIT;

