-- @pgevolve plan id=4632bce09f2d4081 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_procedure destructive=true intent_id=1 targets=app.old_proc
DROP PROCEDURE app.old_proc;
COMMIT;

