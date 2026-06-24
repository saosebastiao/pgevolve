-- @pgevolve plan id=96edc223b9a1d9cf version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.role
COMMENT ON TYPE app.role IS 'User roles for access control';
COMMIT;

