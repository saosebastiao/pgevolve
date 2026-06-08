-- @pgevolve plan id=e55ffaf743dd4d1c version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_index_storage destructive=false targets=app.i
ALTER INDEX app.i SET (fastupdate = false);
COMMIT;

