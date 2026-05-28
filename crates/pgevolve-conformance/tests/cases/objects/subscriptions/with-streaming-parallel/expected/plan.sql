-- @pgevolve plan id=5b07da9b2276d1fe version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_publication destructive=false targets=
CREATE PUBLICATION p FOR ALL TABLES;
-- @pgevolve step=2 kind=create_subscription destructive=false targets=
CREATE SUBSCRIPTION s CONNECTION 'host=replica.example.com dbname=app user=repl password=${REPL_PWD}' PUBLICATION p WITH (streaming = parallel);
COMMIT;

