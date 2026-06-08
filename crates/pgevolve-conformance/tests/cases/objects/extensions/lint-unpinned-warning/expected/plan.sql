-- @pgevolve plan id=41876fa403428a5e version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_extension destructive=false targets=pg_extension.pgcrypto
CREATE EXTENSION IF NOT EXISTS pgcrypto;
COMMIT;

