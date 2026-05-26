-- @pgevolve plan id=cbd881772be90aef version=0.3.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=set_materialized_view_storage destructive=false targets=app.m
ALTER MATERIALIZED VIEW app.m SET (fillfactor = 90);
COMMIT;

