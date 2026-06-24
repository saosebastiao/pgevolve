-- @pgevolve plan id=4a1c0c9e00f1e4f7 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.role
COMMENT ON TYPE app.role IS 'User roles for access control';
COMMIT;

