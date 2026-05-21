-- @pgevolve plan id=f2a316c0c8772f27 version=0.1.0-dev ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_trigger destructive=false targets=app.trg_accounts_validate
DROP TRIGGER trg_accounts_validate ON app.accounts;
COMMIT;

