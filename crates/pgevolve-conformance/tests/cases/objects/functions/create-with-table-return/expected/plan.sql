-- @pgevolve plan id=ab01b7ad3e5f5136 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.get_summary
CREATE OR REPLACE FUNCTION app.get_summary()
    RETURNS TABLE(id integer, label text)
    LANGUAGE sql STABLE
AS $pgevolve$SELECT 1, 'first' UNION ALL SELECT 2, 'second'$pgevolve$;
COMMIT;

