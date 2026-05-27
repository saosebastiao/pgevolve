-- @pgevolve plan id=f7b05d9fdb7b1b4e version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_statistic destructive=false targets=app.t_lower
CREATE STATISTICS app.t_lower ON (lower(name)) FROM app.t;
COMMIT;

