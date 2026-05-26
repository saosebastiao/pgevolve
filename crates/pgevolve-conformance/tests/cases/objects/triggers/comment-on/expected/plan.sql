-- @pgevolve plan id=d9dd6756b3775329 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_trigger destructive=false targets=app.trg_notifications_send
COMMENT ON TRIGGER trg_notifications_send ON app.notifications IS 'fires after each row insert to dispatch a notification';
COMMIT;

