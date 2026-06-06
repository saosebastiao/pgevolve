-- @pgevolve plan id=471247a9013090ba version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION p FOR ALL TABLES;
COMMIT;

-- @pgevolve group id=2 transactional=false
-- @pgevolve step=2 kind=create_subscription destructive=false targets=
CREATE SUBSCRIPTION s CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}' PUBLICATION p WITH (failover = true);

