-- @pgevolve plan id=261d4748cf5036c2 version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_domain_set_not_null destructive=false targets=app.nonnull_int
ALTER DOMAIN app.nonnull_int SET NOT NULL;
COMMIT;

