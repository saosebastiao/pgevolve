-- @pgevolve plan id=ec09a4a96a4191e7 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_procedure destructive=false targets=app.greet
COMMENT ON PROCEDURE app.greet IS 'Prints a greeting notice';
COMMIT;

