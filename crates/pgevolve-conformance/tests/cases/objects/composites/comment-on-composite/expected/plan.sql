-- @pgevolve plan id=a9d468ba4a965201 version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_type destructive=false targets=app.address
COMMENT ON TYPE app.address IS 'Postal address composite type';
COMMIT;

