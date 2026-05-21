-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.shipments (
    id           bigint  NOT NULL,
    shipped_at   date    NOT NULL,
    weight_kg    numeric NOT NULL
) PARTITION BY RANGE (shipped_at);
CREATE TABLE app.shipments_2024
    PARTITION OF app.shipments
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
