-- @pgevolve plan id=992237949989562f version=0.2.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_column destructive=true intent_id=1 targets=app.users
ALTER TABLE app.users DROP COLUMN notes;
COMMIT;

