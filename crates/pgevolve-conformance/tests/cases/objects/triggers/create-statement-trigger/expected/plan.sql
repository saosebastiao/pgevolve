-- @pgevolve plan id=524cae16ac7a3c3c version=0.3.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_events_log
CREATE TRIGGER trg_events_log AFTER UPDATE ON app.events FOR EACH STATEMENT EXECUTE FUNCTION app.log_statement();
COMMIT;

