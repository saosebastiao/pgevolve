-- @pgevolve plan id=8af6f2c548659323 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_object_owner destructive=false targets=app.t
ALTER TABLE app.t OWNER TO app_owner;
COMMIT;

