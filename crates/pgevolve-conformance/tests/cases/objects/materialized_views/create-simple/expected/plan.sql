-- @pgevolve plan id=08503696dd4f4491 version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_materialized_view destructive=false targets=app.user_stats
CREATE MATERIALIZED VIEW app.user_stats (user_id, event_count) AS
SELECT user_id, count(*) AS event_count FROM app.events GROUP BY user_id
WITH NO DATA;
-- @pgevolve step=2 kind=refresh_materialized_view destructive=false targets=app.user_stats
REFRESH MATERIALIZED VIEW app.user_stats;
COMMIT;

