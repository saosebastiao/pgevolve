-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY, amount numeric);
CREATE PUBLICATION large_orders FOR TABLE app.orders WHERE (id > 1000);
