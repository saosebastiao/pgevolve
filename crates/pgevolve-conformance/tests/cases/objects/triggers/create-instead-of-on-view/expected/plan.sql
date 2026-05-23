-- @pgevolve plan id=cb5ec07308036ce7 version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_trigger destructive=false targets=app.trg_users_view_insert
CREATE TRIGGER trg_users_view_insert INSTEAD OF INSERT ON app.users_view FOR EACH ROW EXECUTE FUNCTION app.upsert_user();
COMMIT;

