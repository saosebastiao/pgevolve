-- @pgevolve plan id=2ea59ae44a33dd90 version=0.3.7 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_table destructive=true intent_id=1 targets=app.legacy
DROP TABLE app.legacy;
COMMIT;

