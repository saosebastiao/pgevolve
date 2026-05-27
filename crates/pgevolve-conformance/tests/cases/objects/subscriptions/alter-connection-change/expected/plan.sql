-- @pgevolve plan id=c1f4f1d28139be92 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_subscription_connection destructive=false targets=
ALTER SUBSCRIPTION s CONNECTION 'host=replica.example.com dbname=app user=repl password=${PWD_B}';
COMMIT;

