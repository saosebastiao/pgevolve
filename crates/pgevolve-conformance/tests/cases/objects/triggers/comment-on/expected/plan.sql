-- @pgevolve plan id=aad279ad9076efc6 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_trigger destructive=false targets=app.trg_notifications_send
COMMENT ON TRIGGER trg_notifications_send ON app.notifications IS 'fires after each row insert to dispatch a notification';
COMMIT;

