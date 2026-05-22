-- @pgevolve plan id=e3b283a82689d739 version=0.3.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_extension destructive=false targets=pg_extension.pg_trgm
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA app;
COMMIT;

