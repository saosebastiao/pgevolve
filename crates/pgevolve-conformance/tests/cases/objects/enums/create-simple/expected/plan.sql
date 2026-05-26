-- @pgevolve plan id=79bb0f4ec698e49a version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.status
CREATE TYPE app.status AS ENUM ('active', 'inactive', 'pending');
COMMIT;

