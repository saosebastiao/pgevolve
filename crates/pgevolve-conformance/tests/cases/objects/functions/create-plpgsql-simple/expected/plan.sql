-- @pgevolve plan id=8f48cefa4e095c34 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.triple
CREATE OR REPLACE FUNCTION app.triple(n integer)
    RETURNS integer
    LANGUAGE plpgsql IMMUTABLE STRICT
AS $pgevolve$BEGIN RETURN n * 3; END$pgevolve$;
COMMIT;

