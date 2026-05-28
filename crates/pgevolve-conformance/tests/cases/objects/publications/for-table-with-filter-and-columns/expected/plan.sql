-- @pgevolve plan id=060942d28f9dbe9c version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION orders_filtered FOR TABLE app.orders (amount, id) WHERE (id > 1000);
COMMIT;

