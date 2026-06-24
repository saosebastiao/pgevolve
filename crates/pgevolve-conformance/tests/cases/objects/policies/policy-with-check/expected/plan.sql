-- @pgevolve plan id=01e2f8f38266da89 version=0.4.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_policy destructive=false targets=app.docs
CREATE POLICY p ON app.docs AS PERMISSIVE FOR INSERT TO PUBLIC WITH CHECK (true);
COMMIT;

