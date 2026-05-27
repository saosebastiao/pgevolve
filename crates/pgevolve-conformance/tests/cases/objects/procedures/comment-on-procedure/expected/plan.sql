-- @pgevolve plan id=cf6ae0f9c68d44cb version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=comment_on_procedure destructive=false targets=app.greet
COMMENT ON PROCEDURE app.greet IS 'Prints a greeting notice';
COMMIT;

