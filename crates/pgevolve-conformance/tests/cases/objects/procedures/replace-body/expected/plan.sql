-- @pgevolve plan id=adeca1b25b06e645 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_procedure destructive=false targets=app.greet
CREATE OR REPLACE PROCEDURE app.greet()
    LANGUAGE plpgsql
AS $pgevolve$BEGIN RAISE NOTICE 'Hello, world!'; END$pgevolve$;
COMMIT;

