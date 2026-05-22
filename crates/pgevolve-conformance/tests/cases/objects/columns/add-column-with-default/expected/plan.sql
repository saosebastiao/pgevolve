-- @pgevolve plan id=7b427b046231b9b8 version=0.3.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.products
ALTER TABLE app.products ADD COLUMN quantity integer DEFAULT 0;
COMMIT;

