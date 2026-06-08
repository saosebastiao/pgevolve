-- @pgevolve plan id=459c23370a7c723b version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=false
-- @pgevolve step=1 kind=create_subscription destructive=false targets=
CREATE SUBSCRIPTION s CONNECTION 'host=x dbname=app user=repl password=hunter2' PUBLICATION p;

