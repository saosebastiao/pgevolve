-- @pgevolve plan id=c3f921dcdc57a98e version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_ghost_insert
CREATE TRIGGER trg_ghost_insert AFTER INSERT ON app.ghost_table FOR EACH ROW EXECUTE FUNCTION app.on_ghost_insert();
COMMIT;

