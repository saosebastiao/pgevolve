-- @pgevolve plan id=e82a0a20c9b41176 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_publication_drop_table destructive=false targets=app.customers
ALTER PUBLICATION main DROP TABLE app.customers;
COMMIT;

