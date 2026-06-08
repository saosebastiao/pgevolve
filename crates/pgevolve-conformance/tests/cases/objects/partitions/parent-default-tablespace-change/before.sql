-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.metrics (
    id         bigint NOT NULL,
    recorded_at date   NOT NULL,
    value       numeric NOT NULL
) PARTITION BY RANGE (recorded_at);
