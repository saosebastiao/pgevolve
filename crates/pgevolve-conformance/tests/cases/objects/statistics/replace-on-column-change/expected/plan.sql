-- @pgevolve plan id=aadbd7f8c177b553 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=replace_statistic destructive=true intent_id=1 targets=app.s
DROP STATISTICS app.s;
CREATE STATISTICS app.s (ndistinct) ON a, b, c FROM app.t;
COMMIT;

