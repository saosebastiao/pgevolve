-- @pgevolve plan id=c51dc740a40120b2 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION orders_slim FOR TABLE app.orders (id, status);
COMMIT;

