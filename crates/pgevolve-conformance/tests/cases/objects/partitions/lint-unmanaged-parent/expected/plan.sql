-- @pgevolve plan id=3d2c44bd3cf28efd version=0.3.2 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.orders_2024
CREATE TABLE app.orders_2024 PARTITION OF app.external_orders FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
COMMIT;

