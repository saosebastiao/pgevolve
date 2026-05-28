-- @pgevolve plan id=0dd16b5ab0bda736 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_policy destructive=false targets=app.docs
CREATE POLICY p ON app.docs AS PERMISSIVE FOR ALL TO readers USING (true);
COMMIT;

