-- @pgevolve plan id=427ae33b59c1612b version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=detach_partition destructive=false targets=app.metrics_2024
ALTER TABLE app.metrics DETACH PARTITION app.metrics_2024;
COMMIT;

