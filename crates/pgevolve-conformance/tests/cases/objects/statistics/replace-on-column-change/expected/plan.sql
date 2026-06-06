-- @pgevolve plan id=df7c3857022d428b version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=replace_statistic destructive=true intent_id=1 targets=app.s
DROP STATISTICS app.s;
CREATE STATISTICS app.s (ndistinct) ON a, b, c FROM app.t;
COMMIT;

