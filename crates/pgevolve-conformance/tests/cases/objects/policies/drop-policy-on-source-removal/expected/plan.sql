-- @pgevolve plan id=e95ff197dd6b1c39 version=0.4.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_policy destructive=false targets=app.docs
DROP POLICY p ON app.docs;
COMMIT;

