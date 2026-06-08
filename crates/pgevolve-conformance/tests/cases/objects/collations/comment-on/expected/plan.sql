-- @pgevolve plan id=fbe618baaa99faa3 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_collation destructive=false targets=app.c_libc
COMMENT ON COLLATION app.c_libc IS 'pinned for binary sorting';
COMMIT;

