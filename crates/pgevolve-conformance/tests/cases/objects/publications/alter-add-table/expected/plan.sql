-- @pgevolve plan id=9b89199da6a2a60a version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_publication_add_table destructive=false targets=app.customers
ALTER PUBLICATION main ADD TABLE app.customers;
COMMIT;

