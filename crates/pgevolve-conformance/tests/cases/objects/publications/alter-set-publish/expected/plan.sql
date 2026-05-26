-- @pgevolve plan id=c7df84182305d53b version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_publication_set_publish destructive=false targets=
ALTER PUBLICATION main SET (publish = 'insert, update');
COMMIT;

