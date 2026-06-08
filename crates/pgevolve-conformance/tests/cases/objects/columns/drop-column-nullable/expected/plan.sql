-- @pgevolve plan id=2e3674d4fe5a053f version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_column destructive=true intent_id=1 targets=app.users
ALTER TABLE app.users DROP COLUMN notes;
COMMIT;

