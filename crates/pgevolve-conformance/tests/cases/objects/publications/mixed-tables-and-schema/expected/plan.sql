-- @pgevolve plan id=386d6a00e651724f version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION mixed_pub FOR TABLE app.orders, TABLES IN SCHEMA billing;
COMMIT;

