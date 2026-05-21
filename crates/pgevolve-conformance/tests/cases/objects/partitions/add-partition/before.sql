-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.invoices (
    id           bigint NOT NULL,
    invoiced_at  date   NOT NULL,
    amount       numeric NOT NULL
) PARTITION BY RANGE (invoiced_at);
CREATE TABLE app.invoices_2024
    PARTITION OF app.invoices
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
