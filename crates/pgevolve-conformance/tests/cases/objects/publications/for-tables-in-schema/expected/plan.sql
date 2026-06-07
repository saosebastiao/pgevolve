-- @pgevolve plan id=2f3f69f6a793e6e6 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION schema_pub FOR TABLES IN SCHEMA app, billing;
COMMIT;

