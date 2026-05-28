-- @pgevolve plan id=a43cf6d3f8cc340b version=0.3.7 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_policy destructive=false targets=app.docs
CREATE POLICY p ON app.docs AS PERMISSIVE FOR INSERT TO PUBLIC WITH CHECK (true);
COMMIT;

