-- @pgevolve plan id=9ff19b766a470d9d version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.email
CREATE DOMAIN app.email AS text;
COMMIT;

