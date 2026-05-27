-- @pgevolve plan id=7ba345358bf5f99c version=0.3.5 ruleset=1
-- @pgevolve target=conformance-test-target
-- @pgevolve intents_required=0

-- @pgevolve group id=1 transactional=true
BEGIN;
-- @pgevolve step=1 kind=create_table destructive=false targets=app.transactions
CREATE TABLE app.transactions (
    id bigint NOT NULL,
    region text NOT NULL,
    txn_date date NOT NULL,
    amount numeric NOT NULL
) PARTITION BY LIST (region);
-- @pgevolve step=2 kind=create_table destructive=false targets=app.transactions_emea
CREATE TABLE app.transactions_emea PARTITION OF app.transactions FOR VALUES IN ('emea') PARTITION BY RANGE (txn_date);
-- @pgevolve step=3 kind=create_table destructive=false targets=app.transactions_emea_2024
CREATE TABLE app.transactions_emea_2024 PARTITION OF app.transactions_emea FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
-- @pgevolve step=4 kind=create_table destructive=false targets=app.transactions_emea_2025
CREATE TABLE app.transactions_emea_2025 PARTITION OF app.transactions_emea FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
COMMIT;

