-- @pgevolve plan id=5c44d9a149bd958e version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_constraint destructive=true intent_id=1 targets=app.products
ALTER TABLE app.products DROP CONSTRAINT products_price_positive;
COMMIT;

