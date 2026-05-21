-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.transactions (
    id          bigint NOT NULL,
    region      text   NOT NULL,
    txn_date    date   NOT NULL,
    amount      numeric NOT NULL
) PARTITION BY LIST (region);
CREATE TABLE app.transactions_emea
    PARTITION OF app.transactions
    FOR VALUES IN ('emea')
    PARTITION BY RANGE (txn_date);
CREATE TABLE app.transactions_emea_2024
    PARTITION OF app.transactions_emea
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE TABLE app.transactions_emea_2025
    PARTITION OF app.transactions_emea
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
