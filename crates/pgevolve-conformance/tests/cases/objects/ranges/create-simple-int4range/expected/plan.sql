-- @pgevolve plan id=d0154220e8442d2f version=0.3.7 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.int_window
CREATE TYPE app.int_window AS RANGE (subtype = pg_catalog.int4);
COMMIT;

