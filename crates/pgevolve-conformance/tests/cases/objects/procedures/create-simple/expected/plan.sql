-- @pgevolve plan id=aa2288595c5d6097 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_procedure destructive=false targets=app.do_nothing
CREATE OR REPLACE PROCEDURE app.do_nothing()
    LANGUAGE plpgsql
AS $pgevolve$BEGIN NULL; END$pgevolve$;
COMMIT;

