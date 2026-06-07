-- @pgevolve plan id=90840e4521e6868b version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION large_orders FOR TABLE app.orders WHERE (id > 1000);
COMMIT;

