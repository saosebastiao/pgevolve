-- @pgevolve plan id=f2d0d67a62a5d31f version=0.4.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.float8_diff
CREATE OR REPLACE FUNCTION app.float8_diff(double precision, double precision)
    RETURNS double precision
    LANGUAGE sql IMMUTABLE STRICT
AS $pgevolve$SELECT $1 - $2$pgevolve$;
-- @pgevolve step=2 kind=create_type destructive=false targets=app.float8_range
CREATE TYPE app.float8_range AS RANGE (subtype = pg_catalog.float8, subtype_diff = app.float8_diff);
COMMIT;

