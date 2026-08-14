-- @pgevolve plan id=50b5c8f8ca7c5444 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_constraint destructive=false targets=app.orders
ALTER TABLE app.orders ADD CONSTRAINT orders_amount_positive CHECK (amount > 0) NOT ENFORCED;
COMMIT;

