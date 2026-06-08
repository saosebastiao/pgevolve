-- @pgevolve plan id=a59654e21c3b8aee version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_cast destructive=false targets=app.label,app.tag
CREATE CAST (app.label AS app.tag) WITH INOUT;
COMMIT;

