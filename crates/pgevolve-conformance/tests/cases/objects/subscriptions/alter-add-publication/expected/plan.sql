-- @pgevolve plan id=6539411d2063b7b1 version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_subscription_add_publication destructive=false targets=
ALTER SUBSCRIPTION s ADD PUBLICATION p2;
COMMIT;

