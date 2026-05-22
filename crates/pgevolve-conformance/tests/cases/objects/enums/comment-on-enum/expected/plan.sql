-- @pgevolve plan id=4839b7a3fdb21af6 version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.role
COMMENT ON TYPE app.role IS 'User roles for access control';
COMMIT;

