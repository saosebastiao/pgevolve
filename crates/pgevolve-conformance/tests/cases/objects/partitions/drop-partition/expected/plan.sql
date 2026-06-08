-- @pgevolve plan id=87c689e812c8e689 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_table destructive=true intent_id=1 targets=app.shipments_2023
DROP TABLE app.shipments_2023;
COMMIT;

