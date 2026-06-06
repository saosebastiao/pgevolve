-- @pgevolve plan id=af4c5c436a99d3df version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_extension destructive=false targets=pg_extension.pgcrypto
CREATE EXTENSION IF NOT EXISTS pgcrypto;
COMMIT;

