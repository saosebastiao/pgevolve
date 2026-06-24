-- @pgevolve plan id=6bd1cf9180542c7b version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION mixed_pub FOR TABLE app.orders, TABLES IN SCHEMA billing;
COMMIT;

