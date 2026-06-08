-- @pgevolve plan id=2bf6b56357153367 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_extension destructive=false targets=pg_extension.pgcrypto
CREATE EXTENSION IF NOT EXISTS pgcrypto;
COMMIT;

