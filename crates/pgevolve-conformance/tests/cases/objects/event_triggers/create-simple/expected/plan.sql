-- @pgevolve plan id=9e1f732975ce5340 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_event_trigger destructive=false targets=pg_event_trigger.et_audit
CREATE EVENT TRIGGER et_audit ON ddl_command_end EXECUTE FUNCTION app.audit();
COMMIT;

