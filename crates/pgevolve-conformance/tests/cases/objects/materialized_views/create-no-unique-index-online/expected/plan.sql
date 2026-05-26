-- @pgevolve plan id=7eb5ef8393c83c1c version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_materialized_view destructive=false targets=app.nolynx_report
CREATE MATERIALIZED VIEW app.nolynx_report (id, amount) AS
SELECT id, amount FROM app.orders
WITH NO DATA;
-- @pgevolve step=2 kind=refresh_materialized_view destructive=false targets=app.nolynx_report
REFRESH MATERIALIZED VIEW app.nolynx_report;
COMMIT;

