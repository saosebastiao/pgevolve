-- @pgevolve plan id=31f21f2977b16939 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_cast destructive=false targets=pg_catalog.int4,pg_catalog.text
CREATE CAST (pg_catalog.int4 AS pg_catalog.text) WITH FUNCTION app.ghost_cast_fn(integer);
COMMIT;

