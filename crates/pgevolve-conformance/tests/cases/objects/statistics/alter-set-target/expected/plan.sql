-- @pgevolve plan id=65a26f5969a2a9de version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_statistic_set_target destructive=false targets=app.orders_corr
ALTER STATISTICS app.orders_corr SET STATISTICS 1000;
COMMIT;

