-- @pgevolve plan id=04154a5f44a1f789 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_column destructive=false targets=app.products
ALTER TABLE app.products ADD COLUMN quantity integer DEFAULT 0;
COMMIT;

