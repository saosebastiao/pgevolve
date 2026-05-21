-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.readings (
    id          bigint NOT NULL,
    recorded_at date   NOT NULL,
    value       numeric NOT NULL
) PARTITION BY RANGE (recorded_at);
CREATE TABLE app.readings_2024 (
    id          bigint NOT NULL,
    recorded_at date   NOT NULL,
    value       numeric NOT NULL
);
