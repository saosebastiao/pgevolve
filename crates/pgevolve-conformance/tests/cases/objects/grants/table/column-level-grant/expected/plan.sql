-- @pgevolve plan id=9a88100bc8261d61 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_column_privilege destructive=false targets=app.t
GRANT INSERT (name) ON TABLE app.t TO readers;
COMMIT;

