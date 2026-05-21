-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
    id         bigint  NOT NULL,
    placed_at  date    NOT NULL,
    total      numeric NOT NULL
) PARTITION BY RANGE (placed_at);
CREATE TABLE app.orders_2024
    PARTITION OF app.orders
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE TABLE app.orders_2025
    PARTITION OF app.orders
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
