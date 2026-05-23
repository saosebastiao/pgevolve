-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.parent (id bigint, region text) PARTITION BY LIST (region);
CREATE TABLE app.child_us PARTITION OF app.parent
    FOR VALUES IN ('us');
