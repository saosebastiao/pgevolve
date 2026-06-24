-- @pgevolve plan id=5059f02aa7979d2a version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION orders_slim FOR TABLE app.orders (id, status);
COMMIT;

