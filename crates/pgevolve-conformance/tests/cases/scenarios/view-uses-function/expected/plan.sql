-- @pgevolve plan id=4f6b0a1192a449ee version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.products_with_tax
CREATE VIEW app.products_with_tax (id, price, tax) AS
SELECT id, price, price * app.tax_rate() AS tax FROM app.products;
-- @pgevolve step=2 kind=create_or_replace_function destructive=false targets=app.tax_rate
CREATE OR REPLACE FUNCTION app.tax_rate()
    RETURNS numeric
    LANGUAGE sql IMMUTABLE
AS $pgevolve$SELECT 0.1$pgevolve$;
COMMIT;

