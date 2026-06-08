-- @pgevolve plan id=3016900a6d3667f9 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_aggregate destructive=false targets=app.my_sum
CREATE AGGREGATE app.my_sum(integer) (SFUNC = app.sum_sfunc, STYPE = bigint, FINALFUNC = app.sum_ffunc);
COMMIT;

