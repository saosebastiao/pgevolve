-- @pgevolve plan id=3a0dfe6d3b582b11 version=0.4.2 ruleset=1
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

