-- @pgevolve plan id=27f69f5daea93bfc version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.username
COMMENT ON DOMAIN app.username IS 'Validated username string';
COMMIT;

