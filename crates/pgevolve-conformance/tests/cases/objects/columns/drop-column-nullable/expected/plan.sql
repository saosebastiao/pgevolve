-- @pgevolve plan id=c3ee09e821cd0ad0 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=1

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_column destructive=true intent_id=1 targets=app.users
ALTER TABLE app.users DROP COLUMN notes;
COMMIT;

