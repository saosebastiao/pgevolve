-- @pgevolve plan id=5cd10fa809503d66 version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_domain_set_default destructive=false targets=app.quantity
ALTER DOMAIN app.quantity SET DEFAULT 1;
COMMIT;

