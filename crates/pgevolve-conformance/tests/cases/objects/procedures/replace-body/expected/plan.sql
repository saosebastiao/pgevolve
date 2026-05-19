-- @pgevolve plan id=8d58e21f5f2ec2fe version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_procedure destructive=false targets=app.greet
CREATE OR REPLACE PROCEDURE app.greet()
    LANGUAGE plpgsql
AS $pgevolve$BEGIN RAISE NOTICE 'Hello, world!'; END$pgevolve$;
COMMIT;

