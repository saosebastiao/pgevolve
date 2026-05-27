-- @pgevolve plan id=0e9d6939727f5a22 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.products
ALTER TABLE app.products ADD COLUMN quantity integer DEFAULT 0;
COMMIT;

