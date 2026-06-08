-- @pgevolve plan id=e2b8e5c8b4f4b297 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_storage destructive=false targets=app.t
ALTER TABLE app.t SET (fillfactor = 80);
COMMIT;

