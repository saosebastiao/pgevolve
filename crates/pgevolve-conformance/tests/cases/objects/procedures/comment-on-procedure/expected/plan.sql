-- @pgevolve plan id=3a76321c0d3f3116 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_procedure destructive=false targets=app.greet
COMMENT ON PROCEDURE app.greet IS 'Prints a greeting notice';
COMMIT;

