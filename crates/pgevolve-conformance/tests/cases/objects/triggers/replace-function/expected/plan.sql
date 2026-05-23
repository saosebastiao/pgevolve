-- @pgevolve plan id=18b321d675be3d7c version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_trigger destructive=false targets=app.trg_sessions_hook
DROP TRIGGER trg_sessions_hook ON app.sessions;
-- @pgevolve step=2 kind=create_trigger destructive=false targets=app.trg_sessions_hook
CREATE TRIGGER trg_sessions_hook AFTER INSERT ON app.sessions FOR EACH ROW EXECUTE FUNCTION app.fn_b();
COMMIT;

