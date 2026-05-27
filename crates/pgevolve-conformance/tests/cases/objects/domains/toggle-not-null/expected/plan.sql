-- @pgevolve plan id=1e03418a0a4e29d2 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_domain_set_not_null destructive=false targets=app.nonnull_int
ALTER DOMAIN app.nonnull_int SET NOT NULL;
COMMIT;

