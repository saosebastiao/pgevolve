-- @pgevolve plan id=cd98e5ead1105442 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_policy destructive=false targets=app.docs
CREATE POLICY p ON app.docs AS PERMISSIVE FOR ALL TO readers USING (true);
COMMIT;

