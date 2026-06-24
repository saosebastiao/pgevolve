-- @pgevolve plan id=c96276d618fd1cb7 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_type destructive=true intent_id=1 targets=app.int_window
DROP TYPE app.int_window;
COMMIT;

