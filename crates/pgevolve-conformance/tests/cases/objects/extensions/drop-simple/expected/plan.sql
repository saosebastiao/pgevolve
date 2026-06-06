-- @pgevolve plan id=3db00f173ef76ef2 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_extension destructive=true intent_id=1 targets=pg_extension.pgcrypto
DROP EXTENSION pgcrypto CASCADE;
COMMIT;

