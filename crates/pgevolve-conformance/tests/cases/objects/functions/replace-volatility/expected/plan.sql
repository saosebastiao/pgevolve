-- @pgevolve plan id=63079ec8fb545fb2 version=0.2.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.now_plus
CREATE OR REPLACE FUNCTION app.now_plus(n integer)
    RETURNS timestamp
    LANGUAGE sql STRICT
AS $pgevolve$SELECT now() + CAST(n || ' days' AS interval)$pgevolve$;
COMMIT;

