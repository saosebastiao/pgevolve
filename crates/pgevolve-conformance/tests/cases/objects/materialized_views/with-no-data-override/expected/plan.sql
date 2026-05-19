-- @pgevolve plan id=12d2552447b6e73a version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_materialized_view destructive=false targets=app.daily_metrics
CREATE MATERIALIZED VIEW app.daily_metrics (event_date, total) AS
SELECT event_date, sum(metric_value) AS total FROM app.events GROUP BY event_date
WITH NO DATA;
-- @pgevolve step=2 kind=refresh_materialized_view destructive=false targets=app.daily_metrics
REFRESH MATERIALIZED VIEW app.daily_metrics;
COMMIT;

