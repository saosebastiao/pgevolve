-- @pgevolve plan id=d0cbc065ad090eb6 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_schema destructive=false targets=app.app
CREATE SCHEMA app;
-- @pgevolve step=2 kind=create_extension destructive=false targets=pg_extension.pg_trgm
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA app;
COMMIT;

