-- @pgevolve plan id=7b28f5d34baab9ef version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_widgets_hook
CREATE TRIGGER trg_widgets_hook AFTER INSERT ON app.widgets FOR EACH ROW EXECUTE FUNCTION app.ghost_fn();
COMMIT;

