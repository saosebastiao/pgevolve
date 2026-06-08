-- @pgevolve plan id=a5ae406499c740c4 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_object_privilege destructive=false targets=app.foo
GRANT EXECUTE ON FUNCTION app.foo(integer) TO readers;
COMMIT;

