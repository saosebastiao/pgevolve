-- @pgevolve plan id=6bcf6c541ff66a9d version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_publication_add_table destructive=false targets=app.customers
ALTER PUBLICATION main ADD TABLE app.customers;
COMMIT;

