-- @pgevolve plan id=6a9ff2ca5c8fd278 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_procedure destructive=true intent_id=1 targets=app.old_proc
DROP PROCEDURE app.old_proc;
COMMIT;

