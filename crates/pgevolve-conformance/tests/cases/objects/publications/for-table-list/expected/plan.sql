-- @pgevolve plan id=c08f601b8b11db2c version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION main FOR TABLE app.customers, app.orders;
COMMIT;

