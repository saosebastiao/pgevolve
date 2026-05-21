-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
    id      bigint NOT NULL,
    region  text   NOT NULL,
    name    text   NOT NULL
) PARTITION BY LIST (region);
CREATE TABLE app.products_emea
    PARTITION OF app.products
    FOR VALUES IN ('emea');
