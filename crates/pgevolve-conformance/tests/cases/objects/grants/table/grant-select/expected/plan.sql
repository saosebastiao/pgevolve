-- @pgevolve plan id=a131d9dbc72fbd2c version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_object_privilege destructive=false targets=app.t
GRANT SELECT ON TABLE app.t TO readers;
COMMIT;

