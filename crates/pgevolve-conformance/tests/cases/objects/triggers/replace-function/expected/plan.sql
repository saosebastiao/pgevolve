-- @pgevolve plan id=2c5047bc4f0fff3f version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_trigger destructive=false targets=app.trg_sessions_hook
DROP TRIGGER trg_sessions_hook ON app.sessions;
-- @pgevolve step=2 kind=create_trigger destructive=false targets=app.trg_sessions_hook
CREATE TRIGGER trg_sessions_hook AFTER INSERT ON app.sessions FOR EACH ROW EXECUTE FUNCTION app.fn_b();
COMMIT;

