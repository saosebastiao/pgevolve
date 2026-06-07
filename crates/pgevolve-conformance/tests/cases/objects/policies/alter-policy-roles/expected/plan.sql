-- @pgevolve plan id=d7e926856ad4382e version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_policy destructive=false targets=app.docs
ALTER POLICY p ON app.docs TO readers USING (true);
COMMIT;

