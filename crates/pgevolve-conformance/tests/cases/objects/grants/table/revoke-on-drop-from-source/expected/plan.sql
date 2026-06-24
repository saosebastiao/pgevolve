-- @pgevolve plan id=24e1985125ad4bd4 version=0.4.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=revoke_object_privilege destructive=false targets=app.t
REVOKE SELECT ON TABLE app.t FROM readers;
COMMIT;

