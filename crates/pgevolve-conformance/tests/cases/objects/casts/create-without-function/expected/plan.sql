-- @pgevolve plan id=baa8149ac9f43fad version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_cast destructive=false targets=app.user_id,app.account_id
CREATE CAST (app.user_id AS app.account_id) WITHOUT FUNCTION;
COMMIT;

