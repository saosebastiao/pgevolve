-- @pgevolve plan id=1e0857de1b911f71 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_type_alter_attribute_type destructive=true intent_id=1 targets=app.measurement
ALTER TYPE app.measurement ALTER ATTRIBUTE value TYPE bigint;
COMMIT;

