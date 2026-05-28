-- @pgevolve plan id=cb2a7e645129e7e3 version=0.3.7 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_add_attribute destructive=false targets=app.point
ALTER TYPE app.point ADD ATTRIBUTE y numeric;
COMMIT;

