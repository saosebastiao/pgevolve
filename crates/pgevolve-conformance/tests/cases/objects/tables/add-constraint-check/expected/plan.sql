-- @pgevolve plan id=f4c15f7c89eb22a3 version=0.3.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_constraint_not_valid destructive=false targets=app.products
ALTER TABLE app.products ADD CONSTRAINT products_quantity_positive CHECK (quantity > 0) NOT VALID;
-- @pgevolve step=2 kind=validate_constraint destructive=false targets=app.products
ALTER TABLE app.products VALIDATE CONSTRAINT products_quantity_positive;
COMMIT;

