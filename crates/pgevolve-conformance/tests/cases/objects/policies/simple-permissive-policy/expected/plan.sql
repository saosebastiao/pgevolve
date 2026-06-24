-- @pgevolve plan id=71cbf30b48dce127 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_policy destructive=false targets=app.docs
CREATE POLICY author_only ON app.docs AS PERMISSIVE FOR ALL TO PUBLIC USING (author = current_user);
COMMIT;

