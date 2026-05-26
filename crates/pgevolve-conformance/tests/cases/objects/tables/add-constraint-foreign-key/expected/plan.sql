-- @pgevolve plan id=25259328e8c7c0dd version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=add_constraint_not_valid destructive=false targets=app.products
ALTER TABLE app.products ADD CONSTRAINT products_category_id_fkey FOREIGN KEY (category_id) REFERENCES app.categories (id) NOT VALID;
-- @pgevolve step=2 kind=validate_constraint destructive=false targets=app.products
ALTER TABLE app.products VALIDATE CONSTRAINT products_category_id_fkey;
COMMIT;

