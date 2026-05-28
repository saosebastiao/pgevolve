-- @pgevolve plan id=f0099679b0fe2869 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_collation destructive=false targets=app.c_libc
CREATE COLLATION app.c_libc (provider = libc, locale = 'C');
COMMIT;

