-- @pgevolve plan id=250923af54ccbd6d version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_constraint destructive=true intent_id=1 targets=app.products
ALTER TABLE app.products DROP CONSTRAINT products_price_positive;
COMMIT;

