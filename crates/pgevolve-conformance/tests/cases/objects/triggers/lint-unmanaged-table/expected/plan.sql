-- @pgevolve plan id=cf19c34f01b01b2c version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_ghost_insert
CREATE TRIGGER trg_ghost_insert AFTER INSERT ON app.ghost_table FOR EACH ROW EXECUTE FUNCTION app.on_ghost_insert();
COMMIT;

