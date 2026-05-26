-- @pgevolve plan id=6f7e08c5979d33c2 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_object_privilege destructive=false targets=app.app
GRANT USAGE ON SCHEMA app TO readers;
-- @pgevolve step=2 kind=grant_object_privilege destructive=false targets=app.app
GRANT CREATE ON SCHEMA app TO readers;
COMMIT;

