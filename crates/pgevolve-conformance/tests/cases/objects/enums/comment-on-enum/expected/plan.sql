-- @pgevolve plan id=6d95c8f38b8fdc1f version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.role
COMMENT ON TYPE app.role IS 'User roles for access control';
COMMIT;

