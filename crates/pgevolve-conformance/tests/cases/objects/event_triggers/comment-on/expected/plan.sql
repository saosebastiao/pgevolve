-- @pgevolve plan id=911898485747f2aa version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_event_trigger destructive=false targets=pg_event_trigger.et_audit
COMMENT ON EVENT TRIGGER et_audit IS 'audits DDL';
COMMIT;

