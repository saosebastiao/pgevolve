-- @pgevolve plan id=c0c91a77b1f234f0 version=0.2.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.status
CREATE TYPE app.status AS ENUM ('active', 'inactive', 'pending');
COMMIT;

