-- @pgevolve plan id=222904dfa3b184a9 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_index_storage destructive=false targets=app.i
ALTER INDEX app.i SET (pages_per_range = 32);
COMMIT;

