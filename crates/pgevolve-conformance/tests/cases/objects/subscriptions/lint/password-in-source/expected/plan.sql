-- @pgevolve plan id=4fa61bf5dae8285c version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_subscription destructive=false targets=
CREATE SUBSCRIPTION s CONNECTION 'host=x dbname=app user=repl password=hunter2' PUBLICATION p;
COMMIT;

