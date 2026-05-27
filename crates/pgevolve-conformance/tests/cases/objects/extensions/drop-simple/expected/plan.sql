-- @pgevolve plan id=5c31bc96dba0f008 version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_extension destructive=true intent_id=1 targets=pg_extension.pgcrypto
DROP EXTENSION pgcrypto CASCADE;
COMMIT;

