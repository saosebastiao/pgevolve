-- @pgevolve plan id=0d34ecbb09105049 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_subscription_drop_publication destructive=false targets=
ALTER SUBSCRIPTION s DROP PUBLICATION p2;
COMMIT;

