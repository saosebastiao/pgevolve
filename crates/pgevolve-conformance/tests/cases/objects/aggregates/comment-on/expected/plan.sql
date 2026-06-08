-- @pgevolve plan id=e6b737a0097e939d version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_aggregate destructive=false targets=app.my_sum
COMMENT ON AGGREGATE app.my_sum(integer) IS 'sums ints';
COMMIT;

