-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
    id        bigint NOT NULL,
    region    text   NOT NULL,
    amount    numeric NOT NULL
) PARTITION BY RANGE (id);
