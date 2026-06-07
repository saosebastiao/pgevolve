-- @pgevolve plan id=09b7fb32897d37b4 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION orders_filtered FOR TABLE app.orders (amount, id) WHERE (id > 1000);
COMMIT;

