-- @pgevolve plan id=bb7298aef654bea1 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_event_trigger destructive=false targets=pg_event_trigger.et_audit
DROP EVENT TRIGGER et_audit;
-- @pgevolve step=2 kind=create_event_trigger destructive=false targets=pg_event_trigger.et_audit
CREATE EVENT TRIGGER et_audit ON sql_drop EXECUTE FUNCTION app.audit();
COMMIT;

