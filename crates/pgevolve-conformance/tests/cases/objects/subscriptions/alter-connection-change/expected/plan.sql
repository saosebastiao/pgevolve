-- @pgevolve plan id=36712c814aa72323 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_subscription_connection destructive=false targets=
ALTER SUBSCRIPTION s CONNECTION 'host=replica.example.com dbname=app user=repl password=${PWD_B}';
COMMIT;

