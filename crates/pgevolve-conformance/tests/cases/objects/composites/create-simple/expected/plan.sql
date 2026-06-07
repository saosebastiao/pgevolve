-- @pgevolve plan id=4f3aa9b3817575e8 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.address
CREATE TYPE app.address AS (
    street text,
    city text
);
COMMIT;

