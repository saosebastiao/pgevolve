-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.logs (
    id         bigint NOT NULL,
    logged_at  date   NOT NULL,
    message    text   NOT NULL
) PARTITION BY RANGE (logged_at);
CREATE TABLE app.logs_hot
    PARTITION OF app.logs
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01')
    TABLESPACE ts_fast;
