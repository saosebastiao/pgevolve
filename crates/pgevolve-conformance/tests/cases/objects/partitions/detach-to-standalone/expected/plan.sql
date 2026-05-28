-- @pgevolve plan id=23817473a6b0e402 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=detach_partition destructive=false targets=app.metrics_2024
ALTER TABLE app.metrics DETACH PARTITION app.metrics_2024;
COMMIT;

