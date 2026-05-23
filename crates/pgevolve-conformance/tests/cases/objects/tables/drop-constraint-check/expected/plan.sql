-- @pgevolve plan id=1025b25e9ecab6dd version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_constraint destructive=true intent_id=1 targets=app.products
ALTER TABLE app.products DROP CONSTRAINT products_price_positive;
COMMIT;

