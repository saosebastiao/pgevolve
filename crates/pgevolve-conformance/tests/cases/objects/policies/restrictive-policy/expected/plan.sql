-- @pgevolve plan id=8ce8f2bda8bc3997 version=0.3.4 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_policy destructive=false targets=app.docs
CREATE POLICY only_authors ON app.docs AS RESTRICTIVE FOR INSERT TO PUBLIC WITH CHECK (author = current_user);
COMMIT;

