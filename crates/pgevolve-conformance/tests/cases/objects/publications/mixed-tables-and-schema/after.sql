-- @pgevolve schema=app
-- @pgevolve schema=billing
CREATE SCHEMA app;
CREATE SCHEMA billing;
CREATE TABLE app.orders (id bigint PRIMARY KEY);
CREATE TABLE app.customers (id bigint PRIMARY KEY);
CREATE TABLE billing.invoices (id bigint PRIMARY KEY);
CREATE PUBLICATION mixed_pub FOR TABLE app.orders, TABLES IN SCHEMA billing;
