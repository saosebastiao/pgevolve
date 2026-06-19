-- @pgevolve plan id=2e3b8ffba016e361 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_aggregate destructive=false targets=app.my_sum
COMMENT ON AGGREGATE app.my_sum(integer) IS 'sums ints';
COMMIT;

