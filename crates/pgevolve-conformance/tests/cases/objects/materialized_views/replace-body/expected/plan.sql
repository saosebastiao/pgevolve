-- @pgevolve plan id=7744db698fe487fe version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_materialized_view destructive=false targets=app.order_stats
DROP MATERIALIZED VIEW app.order_stats;
-- @pgevolve step=2 kind=create_materialized_view destructive=false targets=app.order_stats
CREATE MATERIALIZED VIEW app.order_stats (customer_id, order_count, total_amount) AS
SELECT customer_id, count(*) AS order_count, sum(amount) AS total_amount FROM app.orders GROUP BY customer_id
WITH NO DATA;
-- @pgevolve step=3 kind=refresh_materialized_view destructive=false targets=app.order_stats
REFRESH MATERIALIZED VIEW app.order_stats;
COMMIT;

