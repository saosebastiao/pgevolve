-- @pgevolve plan id=8e3136f30bbc27f4 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION main FOR TABLE app.customers, app.orders;
COMMIT;

