-- @pgevolve plan id=c190275c9d23dde1 version=0.3.9 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_view_set_reloption destructive=false targets=app.secure_view
ALTER VIEW app.secure_view SET (security_barrier = true);
COMMIT;

