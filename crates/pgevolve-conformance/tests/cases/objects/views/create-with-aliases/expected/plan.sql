-- @pgevolve plan id=3ae7e52d6df67f8c version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_view destructive=false targets=app.aliased_view
CREATE VIEW app.aliased_view (user_id, user_email) AS
SELECT id, email FROM app.users;
COMMIT;

