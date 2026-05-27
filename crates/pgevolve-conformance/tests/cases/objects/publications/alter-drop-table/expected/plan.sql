-- @pgevolve plan id=3612d9be3b32c9f3 version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_publication_drop_table destructive=false targets=app.customers
ALTER PUBLICATION main DROP TABLE app.customers;
COMMIT;

