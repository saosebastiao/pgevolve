-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY);
CREATE TABLE app.customers (id bigint PRIMARY KEY);
CREATE PUBLICATION main FOR TABLE app.orders, app.customers;
