-- @pgevolve plan id=848a9a9ee4dd229c version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.address
COMMENT ON TYPE app.address IS 'Postal address composite type';
COMMIT;

