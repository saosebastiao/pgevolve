-- @pgevolve plan id=694f12544b28fad3 version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.int_window
CREATE TYPE app.int_window AS RANGE (subtype = pg_catalog.int4);
COMMIT;

