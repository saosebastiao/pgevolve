-- @pgevolve plan id=493e2661a1250688 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_items_status_check
CREATE CONSTRAINT TRIGGER trg_items_status_check AFTER INSERT OR UPDATE ON app.items DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION app.check_item_status();
COMMIT;

