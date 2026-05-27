-- @pgevolve plan id=46aae555af7b0663 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.users_summary
CREATE VIEW app.users_summary (id, name) AS
SELECT id, name FROM app.users;
COMMIT;

