-- @pgevolve plan id=13f2f89153a78be6 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_extension destructive=true intent_id=1 targets=pg_extension.pgcrypto
DROP EXTENSION pgcrypto CASCADE;
COMMIT;

