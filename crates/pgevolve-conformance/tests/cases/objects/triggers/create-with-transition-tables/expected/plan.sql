-- @pgevolve plan id=2ef7299a097cd0a3 version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_inventory_sync
CREATE TRIGGER trg_inventory_sync AFTER UPDATE ON app.inventory REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION app.sync_inventory_changes();
COMMIT;

