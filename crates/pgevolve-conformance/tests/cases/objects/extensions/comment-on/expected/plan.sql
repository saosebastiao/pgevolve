-- @pgevolve plan id=9b2e6ee377e5807b version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_extension destructive=false targets=pg_extension.pgcrypto
COMMENT ON EXTENSION pgcrypto IS 'crypto helpers';
COMMIT;

