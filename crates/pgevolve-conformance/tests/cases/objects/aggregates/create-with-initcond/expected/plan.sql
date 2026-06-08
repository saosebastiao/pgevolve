-- @pgevolve plan id=6282e4f3d1662dd9 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_aggregate destructive=false targets=app.my_sum
CREATE AGGREGATE app.my_sum(integer) (SFUNC = app.sum_sfunc, STYPE = bigint, INITCOND = '0');
COMMIT;

