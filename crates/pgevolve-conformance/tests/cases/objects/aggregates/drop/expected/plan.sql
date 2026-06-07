-- @pgevolve plan id=0a96d752f3a7caea version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_aggregate destructive=false targets=app.my_sum
DROP AGGREGATE app.my_sum(integer);
COMMIT;

