-- @pgevolve plan id=84b3ceef62a08215 version=0.4.1 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_policy destructive=false targets=app.docs
CREATE POLICY only_authors ON app.docs AS RESTRICTIVE FOR INSERT TO PUBLIC WITH CHECK (author = current_user);
COMMIT;

