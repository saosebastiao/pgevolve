-- @pgevolve plan id=d9bf4ab71e8cf0e1 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION mixed_pub FOR TABLE app.orders, TABLES IN SCHEMA billing;
COMMIT;

