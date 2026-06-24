-- @pgevolve plan id=6b247aba555531c6 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.get_pairs
CREATE OR REPLACE FUNCTION app.get_pairs()
    RETURNS TABLE(a integer, b text)
    LANGUAGE plpgsql STABLE
AS $pgevolve$BEGIN RETURN QUERY SELECT 1, 'hello'; END$pgevolve$;
COMMIT;

