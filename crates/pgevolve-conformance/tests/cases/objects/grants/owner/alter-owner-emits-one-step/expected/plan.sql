-- @pgevolve plan id=f67d0eb3e7fb4bbf version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_object_owner destructive=false targets=app.t
ALTER TABLE app.t OWNER TO app_owner;
COMMIT;

