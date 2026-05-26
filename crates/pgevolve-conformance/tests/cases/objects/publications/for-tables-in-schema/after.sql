-- @pgevolve schema=app
-- @pgevolve schema=billing
CREATE SCHEMA app;
CREATE SCHEMA billing;
CREATE TABLE app.orders (id bigint PRIMARY KEY);
CREATE TABLE billing.invoices (id bigint PRIMARY KEY);
CREATE PUBLICATION schema_pub FOR TABLES IN SCHEMA app, billing;
