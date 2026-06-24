-- @pgevolve plan id=c00f62e45a8598c2 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_type destructive=false targets=app.int_window
CREATE TYPE app.int_window AS RANGE (subtype = pg_catalog.int4);
COMMIT;

