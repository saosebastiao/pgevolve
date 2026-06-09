-- @pgevolve plan id=16963b4003d02232 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_storage destructive=false targets=app.t
ALTER TABLE app.t SET (fillfactor = 80);
COMMIT;

