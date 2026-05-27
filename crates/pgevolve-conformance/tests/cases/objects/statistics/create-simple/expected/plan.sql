-- @pgevolve plan id=bb1eca7b617799c7 version=0.3.6 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_statistic destructive=false targets=app.orders_corr
CREATE STATISTICS app.orders_corr (ndistinct, dependencies) ON customer_id, status FROM app.orders;
COMMIT;

