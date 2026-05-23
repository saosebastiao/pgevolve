-- @pgevolve plan id=b9b2acf76157a035 version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=alter_default_privileges destructive=false targets=
ALTER DEFAULT PRIVILEGES FOR ROLE app_owner GRANT EXECUTE ON FUNCTIONS TO readers;
COMMIT;

