-- @pgevolve plan id=ea662887755d2de4 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_statistic destructive=false targets=app.s
CREATE STATISTICS app.s (ndistinct, mcv) ON a, b FROM app.t;
COMMIT;

