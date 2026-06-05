-- @pgevolve plan id=70c5b5c2d77d7855 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.products
CREATE TABLE app.products (
    id bigint NOT NULL,
    region text NOT NULL,
    name text NOT NULL
) PARTITION BY LIST (region);
-- @pgevolve step=2 kind=create_table destructive=false targets=app.products_emea
CREATE TABLE app.products_emea PARTITION OF app.products FOR VALUES IN ('emea');
COMMIT;

