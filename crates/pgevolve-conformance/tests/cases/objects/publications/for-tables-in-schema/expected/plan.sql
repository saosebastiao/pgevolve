-- @pgevolve plan id=80f4ce1f561f7e79 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION schema_pub FOR TABLES IN SCHEMA app, billing;
COMMIT;

