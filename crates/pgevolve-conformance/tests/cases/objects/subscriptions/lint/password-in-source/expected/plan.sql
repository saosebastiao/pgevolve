-- @pgevolve plan id=7ebb52078d37ee1b version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=false
-- @pgevolve step=1 kind=create_subscription destructive=false targets=
CREATE SUBSCRIPTION s CONNECTION 'host=x dbname=app user=repl password=hunter2' PUBLICATION p;

