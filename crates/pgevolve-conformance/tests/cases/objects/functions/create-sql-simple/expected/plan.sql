-- @pgevolve plan id=7f5128c2e8ef67e7 version=0.3.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.double
CREATE OR REPLACE FUNCTION app.double(x integer)
    RETURNS integer
    LANGUAGE sql IMMUTABLE STRICT
AS $pgevolve$SELECT x * 2$pgevolve$;
COMMIT;

