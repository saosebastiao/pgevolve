-- @pgevolve plan id=721b9844f42ba9ac version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_materialized_view destructive=false targets=app.node_tree_mv
CREATE MATERIALIZED VIEW app.node_tree_mv (id, depth) AS
WITH RECURSIVE tree(id, depth) AS (SELECT id, 0 AS depth FROM app.nodes WHERE parent_id IS NULL UNION ALL SELECT n.id, t.depth + 1 FROM app.nodes n JOIN tree t ON n.parent_id = t.id) SELECT id, depth FROM tree
WITH NO DATA;
-- @pgevolve step=2 kind=refresh_materialized_view destructive=false targets=app.node_tree_mv
REFRESH MATERIALIZED VIEW app.node_tree_mv;
COMMIT;

