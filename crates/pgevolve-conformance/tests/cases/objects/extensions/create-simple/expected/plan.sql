-- @pgevolve plan id=02c9f973fb63f7cf version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_extension destructive=false targets=pg_extension.pgcrypto
CREATE EXTENSION IF NOT EXISTS pgcrypto;
COMMIT;

