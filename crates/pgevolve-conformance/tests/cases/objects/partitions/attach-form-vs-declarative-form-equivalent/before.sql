-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.fleet (
    id          bigint NOT NULL,
    assigned_at date   NOT NULL,
    vehicle_id  bigint NOT NULL
) PARTITION BY RANGE (assigned_at);
CREATE TABLE app.fleet_2024 (
    id          bigint NOT NULL,
    assigned_at date   NOT NULL,
    vehicle_id  bigint NOT NULL
);
ALTER TABLE app.fleet ATTACH PARTITION app.fleet_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
