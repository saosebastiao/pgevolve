-- @pgevolve plan id=2034ea3e5fa8ae95 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.products
ALTER TABLE app.products ADD COLUMN quantity integer DEFAULT 0;
COMMIT;

