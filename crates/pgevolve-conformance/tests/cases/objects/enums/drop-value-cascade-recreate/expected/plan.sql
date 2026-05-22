-- @pgevolve plan id=46f89f4720674a84 version=0.3.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=2

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_type destructive=true intent_id=1 targets=app.state
DROP TYPE app.state CASCADE;
-- @pgevolve step=2 kind=create_type destructive=true intent_id=2 targets=app.state
CREATE TYPE app.state AS ENUM ('draft', 'archived');
COMMIT;

