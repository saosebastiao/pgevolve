-- @pgevolve plan id=38d4fb05c7563dd9 version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_trigger destructive=false targets=app.trg_products_notify
DROP TRIGGER trg_products_notify ON app.products;
-- @pgevolve step=2 kind=create_trigger destructive=false targets=app.trg_products_notify
CREATE TRIGGER trg_products_notify AFTER INSERT OR UPDATE ON app.products FOR EACH ROW EXECUTE FUNCTION app.notify_change();
COMMIT;

