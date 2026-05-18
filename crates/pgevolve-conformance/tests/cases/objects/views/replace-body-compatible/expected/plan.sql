-- @pgevolve plan id=009d1ff151a40b72 version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.active_users
CREATE OR REPLACE VIEW app.active_users (id, name, email) AS
SELECT id, name, email FROM app.users;
COMMIT;

