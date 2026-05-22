-- @pgevolve plan id=6644cf228754ea55 version=0.2.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_extension destructive=false targets=pg_extension.pgcrypto
COMMENT ON EXTENSION pgcrypto IS 'crypto helpers';
COMMIT;

