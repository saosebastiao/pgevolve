-- @pgevolve plan id=698b1c9ccb6b7337 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_view_set_reloption destructive=false targets=app.invoker_view
ALTER VIEW app.invoker_view SET (security_invoker = true);
COMMIT;

