-- @pgevolve plan id=3c404a688bebb1d5 version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_object_privilege destructive=false targets=app.id_seq
GRANT USAGE ON SEQUENCE app.id_seq TO readers;
COMMIT;

