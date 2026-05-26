-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY, status text, total numeric);
CREATE PUBLICATION orders_slim FOR TABLE app.orders (id, status);
