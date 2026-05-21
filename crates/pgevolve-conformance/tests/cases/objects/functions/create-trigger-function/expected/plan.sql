-- @pgevolve plan id=f02bfac1988d1aec version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_or_replace_function destructive=false targets=app.audit_stamp
CREATE OR REPLACE FUNCTION app.audit_stamp()
    RETURNS trigger
    LANGUAGE plpgsql
AS $pgevolve$BEGIN RETURN NEW; END$pgevolve$;
COMMIT;

