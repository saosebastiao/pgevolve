-- @pgevolve plan id=e5d8402cb971da17 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_collation destructive=true intent_id=1 targets=app.sort
DROP COLLATION app.sort;
-- @pgevolve step=2 kind=create_collation destructive=false targets=app.sort
CREATE COLLATION app.sort (provider = libc, locale = 'POSIX');
COMMIT;

