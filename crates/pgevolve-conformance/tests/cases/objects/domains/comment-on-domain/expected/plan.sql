-- @pgevolve plan id=b3654a76f9ae9f8a version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.username
COMMENT ON DOMAIN app.username IS 'Validated username string';
COMMIT;

