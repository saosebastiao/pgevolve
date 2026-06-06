-- @pgevolve plan id=95870c34577975f3 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_table_storage destructive=false targets=app.t
ALTER TABLE app.t SET (autovacuum_enabled = false);
COMMIT;

