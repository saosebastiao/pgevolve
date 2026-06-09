-- @pgevolve plan id=35a6035b3d244dc4 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION mixed_pub FOR TABLE app.orders, TABLES IN SCHEMA billing;
COMMIT;

