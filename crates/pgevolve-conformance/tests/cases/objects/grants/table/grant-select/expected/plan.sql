-- @pgevolve plan id=7645285d020079c8 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_object_privilege destructive=false targets=app.t
GRANT SELECT ON TABLE app.t TO readers;
COMMIT;

