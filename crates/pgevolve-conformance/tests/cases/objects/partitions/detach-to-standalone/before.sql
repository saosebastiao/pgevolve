-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.metrics (
    id           bigint  NOT NULL,
    collected_at date    NOT NULL,
    value        numeric NOT NULL
) PARTITION BY RANGE (collected_at);
CREATE TABLE app.metrics_2024
    PARTITION OF app.metrics
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
