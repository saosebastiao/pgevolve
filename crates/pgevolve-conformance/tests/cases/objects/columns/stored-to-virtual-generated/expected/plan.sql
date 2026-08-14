-- @pgevolve plan id=6f1cea2688ede59c version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=2

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_column destructive=true intent_id=1 targets=app.items
ALTER TABLE app.items DROP COLUMN doubled;
-- @pgevolve step=2 kind=add_column destructive=true intent_id=2 targets=app.items
ALTER TABLE app.items ADD COLUMN doubled integer GENERATED ALWAYS AS (qty * 2) VIRTUAL;
COMMIT;

