-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.sales (
    id        bigint  NOT NULL,
    sold_at   date    NOT NULL,
    amount    numeric NOT NULL
) PARTITION BY RANGE (sold_at);
CREATE TABLE app.sales_window
    PARTITION OF app.sales
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
