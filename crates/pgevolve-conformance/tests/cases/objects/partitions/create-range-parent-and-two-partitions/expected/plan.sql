-- @pgevolve plan id=c0b7148afe429113 version=0.2.0 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.orders
CREATE TABLE app.orders (
    id bigint NOT NULL,
    placed_at date NOT NULL,
    total numeric NOT NULL
) PARTITION BY RANGE (placed_at);
-- @pgevolve step=2 kind=create_table destructive=false targets=app.orders_2024
CREATE TABLE app.orders_2024 PARTITION OF app.orders FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
-- @pgevolve step=3 kind=create_table destructive=false targets=app.orders_2025
CREATE TABLE app.orders_2025 PARTITION OF app.orders FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
COMMIT;

