-- @pgevolve plan id=2f44e732414bf1ef version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_policy destructive=false targets=app.docs
ALTER POLICY p ON app.docs TO readers USING (true);
COMMIT;

