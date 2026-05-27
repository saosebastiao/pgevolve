-- @pgevolve plan id=972620fc8b920d25 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_view_set_reloption destructive=false targets=app.invoker_view
ALTER VIEW app.invoker_view SET (security_invoker = true);
COMMIT;

