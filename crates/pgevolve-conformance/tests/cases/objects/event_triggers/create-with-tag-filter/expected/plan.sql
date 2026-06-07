-- @pgevolve plan id=4f08a2b599c28b7a version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_event_trigger destructive=false targets=pg_event_trigger.et_audit
CREATE EVENT TRIGGER et_audit ON ddl_command_start WHEN TAG IN ('ALTER TABLE', 'CREATE TABLE') EXECUTE FUNCTION app.audit();
COMMIT;

