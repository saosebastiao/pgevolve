-- @pgevolve plan id=83976b80b54af9d8 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_table_set_comment destructive=false targets=app.users
COMMENT ON TABLE app.users IS 'Application user accounts';
COMMIT;

