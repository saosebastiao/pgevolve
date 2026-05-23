-- @pgevolve plan id=da8b1d2a07c4436c version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_orders_audit
CREATE TRIGGER trg_orders_audit AFTER INSERT ON app.orders FOR EACH ROW EXECUTE FUNCTION app.stamp_audit();
COMMIT;

