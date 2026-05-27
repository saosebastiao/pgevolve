-- @pgevolve plan id=1d037c8809b37d51 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_extension destructive=true intent_id=1 targets=pg_extension.pg_trgm
DROP EXTENSION pg_trgm CASCADE;
-- @pgevolve step=2 kind=create_extension destructive=false targets=pg_extension.pg_trgm
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA gis;
COMMIT;

