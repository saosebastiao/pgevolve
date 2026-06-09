-- @pgevolve plan id=a4ea21f1ed7ca4e8 version=0.4.3 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=grant_object_privilege destructive=false targets=app.t
GRANT SELECT ON TABLE app.t TO readers;
-- @pgevolve step=2 kind=grant_object_privilege destructive=false targets=app.t
GRANT INSERT ON TABLE app.t TO readers;
-- @pgevolve step=3 kind=grant_object_privilege destructive=false targets=app.t
GRANT UPDATE ON TABLE app.t TO readers;
-- @pgevolve step=4 kind=grant_object_privilege destructive=false targets=app.t
GRANT DELETE ON TABLE app.t TO readers;
-- @pgevolve step=5 kind=grant_object_privilege destructive=false targets=app.t
GRANT TRUNCATE ON TABLE app.t TO readers;
-- @pgevolve step=6 kind=grant_object_privilege destructive=false targets=app.t
GRANT REFERENCES ON TABLE app.t TO readers;
-- @pgevolve step=7 kind=grant_object_privilege destructive=false targets=app.t
GRANT TRIGGER ON TABLE app.t TO readers;
COMMIT;

