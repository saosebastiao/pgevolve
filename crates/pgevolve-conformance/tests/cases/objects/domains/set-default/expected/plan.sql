-- @pgevolve plan id=89ee1ced9c2878f5 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_domain_set_default destructive=false targets=app.quantity
ALTER DOMAIN app.quantity SET DEFAULT 1;
COMMIT;

