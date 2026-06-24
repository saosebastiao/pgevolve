-- @pgevolve plan id=63628d63dfd7bbb9 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.double
CREATE OR REPLACE FUNCTION app.double(x integer)
    RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
AS $pgevolve$SELECT x + x$pgevolve$;
COMMIT;

