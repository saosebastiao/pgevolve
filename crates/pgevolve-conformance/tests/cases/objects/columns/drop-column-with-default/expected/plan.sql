-- @pgevolve plan id=2f72e867299290a2 version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_column destructive=true intent_id=1 targets=app.orders
ALTER TABLE app.orders DROP COLUMN status;
COMMIT;

