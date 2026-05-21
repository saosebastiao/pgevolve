-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.readings (
    id          bigint  NOT NULL,
    recorded_at date    NOT NULL,
    value       numeric NOT NULL
) PARTITION BY RANGE (recorded_at);
CREATE TABLE app.readings_2024
    PARTITION OF app.readings
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
