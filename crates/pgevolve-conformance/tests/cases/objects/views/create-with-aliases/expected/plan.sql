-- @pgevolve plan id=cad6ea3ec4cb2d8e version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.aliased_view
CREATE VIEW app.aliased_view (user_id, user_email) AS
SELECT id AS user_id, email AS user_email FROM app.users;
COMMIT;

