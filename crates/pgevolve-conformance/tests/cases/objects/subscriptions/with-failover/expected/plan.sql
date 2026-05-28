-- @pgevolve plan id=fc0e9b1a12a0871a version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION p FOR ALL TABLES;
-- @pgevolve step=2 kind=create_subscription destructive=false targets=
CREATE SUBSCRIPTION s CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}' PUBLICATION p WITH (failover = true);
COMMIT;

