-- @pgevolve plan id=e3b59b75f392d6e5 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_event_trigger_enable destructive=false targets=pg_event_trigger.et_audit
ALTER EVENT TRIGGER et_audit DISABLE;
COMMIT;

