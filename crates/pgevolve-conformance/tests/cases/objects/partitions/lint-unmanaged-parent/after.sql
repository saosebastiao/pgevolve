-- @pgevolve schema=app
CREATE SCHEMA app;
-- app.external_orders is not declared in this project; the
-- partition-references-unmanaged-parent lint (Error severity) fires at CLI
-- plan time. The in-process planner still generates the CREATE TABLE step.
CREATE TABLE app.orders_2024
    PARTITION OF app.external_orders
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
