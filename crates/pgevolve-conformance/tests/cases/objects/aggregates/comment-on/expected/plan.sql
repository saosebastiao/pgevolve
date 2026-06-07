-- @pgevolve plan id=b48d0199b5503e0c version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_aggregate destructive=false targets=app.my_sum
COMMENT ON AGGREGATE app.my_sum(integer) IS 'sums ints';
COMMIT;

