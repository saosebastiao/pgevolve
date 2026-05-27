-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY, customer_id bigint, status text);
CREATE STATISTICS app.orders_corr (ndistinct, dependencies)
    ON customer_id, status
    FROM app.orders;
ALTER STATISTICS app.orders_corr SET STATISTICS 1000;
