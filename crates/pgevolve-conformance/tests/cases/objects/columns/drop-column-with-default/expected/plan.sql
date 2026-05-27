-- @pgevolve plan id=3b9753e95ff65f0d version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_column destructive=true intent_id=1 targets=app.orders
ALTER TABLE app.orders DROP COLUMN status;
COMMIT;

