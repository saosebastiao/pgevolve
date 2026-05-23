-- @pgevolve plan id=7ee7308595aa25e3 version=0.3.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=detach_partition destructive=false targets=app.metrics_2024
ALTER TABLE app.metrics DETACH PARTITION app.metrics_2024;
COMMIT;

