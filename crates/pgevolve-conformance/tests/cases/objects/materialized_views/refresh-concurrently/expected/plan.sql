-- @pgevolve plan id=0811b3dbd15d8781 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_materialized_view destructive=false targets=app.revenue_summary
CREATE MATERIALIZED VIEW app.revenue_summary (region, total) AS
SELECT region, sum(amount) AS total FROM app.sales GROUP BY region
WITH NO DATA;
-- @pgevolve step=2 kind=refresh_materialized_view destructive=false targets=app.revenue_summary
REFRESH MATERIALIZED VIEW app.revenue_summary;
-- @pgevolve step=3 kind=create_index destructive=false targets=app.revenue_summary_region_uidx,app.revenue_summary
CREATE UNIQUE INDEX revenue_summary_region_uidx ON app.revenue_summary USING btree (region);
COMMIT;

