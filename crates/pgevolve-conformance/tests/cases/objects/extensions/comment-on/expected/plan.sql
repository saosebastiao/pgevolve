-- @pgevolve plan id=b04ce8a219cb29e8 version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_extension destructive=false targets=pg_extension.pgcrypto
COMMENT ON EXTENSION pgcrypto IS 'crypto helpers';
COMMIT;

