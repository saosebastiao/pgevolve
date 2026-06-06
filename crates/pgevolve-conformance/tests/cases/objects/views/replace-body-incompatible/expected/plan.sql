-- @pgevolve plan id=a7bb36b51f313019 version=0.4.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_view destructive=false targets=app.user_report
DROP VIEW app.user_report;
-- @pgevolve step=2 kind=create_view destructive=false targets=app.user_report
CREATE VIEW app.user_report (id, name) AS
SELECT id, name FROM app.users;
COMMIT;

