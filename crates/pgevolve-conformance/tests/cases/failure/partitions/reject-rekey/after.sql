-- @pgevolve schema=app
CREATE SCHEMA app;
-- Changing PARTITION BY strategy from RANGE to LIST is not supported in-place.
-- The differ emits UnsupportedDiff which aborts planning.
CREATE TABLE app.orders (
    id        bigint NOT NULL,
    region    text   NOT NULL,
    amount    numeric NOT NULL
) PARTITION BY LIST (region);
