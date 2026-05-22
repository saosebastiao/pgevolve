-- @pgevolve plan id=5fc1bd374b14bd7a version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_add_attribute destructive=false targets=app.point
ALTER TYPE app.point ADD ATTRIBUTE y numeric;
COMMIT;

