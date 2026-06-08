-- @pgevolve plan id=cb9ae9367620c704 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_type destructive=true intent_id=1 targets=app.int_window
DROP TYPE app.int_window;
COMMIT;

