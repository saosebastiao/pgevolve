-- @pgevolve plan id=e0723234ed70b3d1 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_object_privilege destructive=false targets=app.foo
GRANT EXECUTE ON FUNCTION app.foo(integer) TO readers;
COMMIT;

