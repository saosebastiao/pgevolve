-- @pgevolve plan id=debda3029131f7f6 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_column_comment destructive=false targets=app.users
COMMENT ON COLUMN app.users.name IS 'Full display name of the user';
COMMIT;

