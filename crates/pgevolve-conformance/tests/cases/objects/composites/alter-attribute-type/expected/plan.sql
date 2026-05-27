-- @pgevolve plan id=959dd0caf15cf262 version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_alter_attribute_type destructive=true intent_id=1 targets=app.measurement
ALTER TYPE app.measurement ALTER ATTRIBUTE value TYPE bigint;
COMMIT;

