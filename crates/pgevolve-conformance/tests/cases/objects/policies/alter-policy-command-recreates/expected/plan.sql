-- @pgevolve plan id=b9662b7ab2005315 version=0.3.8 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_policy destructive=false targets=app.docs
DROP POLICY p ON app.docs;
-- @pgevolve step=2 kind=create_policy destructive=false targets=app.docs
CREATE POLICY p ON app.docs AS PERMISSIVE FOR INSERT TO PUBLIC WITH CHECK (true);
COMMIT;

