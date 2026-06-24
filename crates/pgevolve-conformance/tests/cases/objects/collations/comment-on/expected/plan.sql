-- @pgevolve plan id=4515f100d8807d6c version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_collation destructive=false targets=app.c_libc
COMMENT ON COLLATION app.c_libc IS 'pinned for binary sorting';
COMMIT;

