-- @pgevolve plan id=d854c63de8d363eb version=0.2.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=drop_trigger destructive=false targets=app.trg_transactions_record
DROP TRIGGER trg_transactions_record ON app.transactions;
-- @pgevolve step=2 kind=create_trigger destructive=false targets=app.trg_transactions_record
CREATE TRIGGER trg_transactions_record AFTER INSERT ON app.transactions FOR EACH ROW WHEN (new.id > 0) EXECUTE FUNCTION app.record_transaction();
COMMIT;

