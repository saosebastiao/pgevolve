-- @pgevolve plan id=1d7fc398f7b4828c version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_subscription_add_publication destructive=false targets=
ALTER SUBSCRIPTION s ADD PUBLICATION p2;
COMMIT;

