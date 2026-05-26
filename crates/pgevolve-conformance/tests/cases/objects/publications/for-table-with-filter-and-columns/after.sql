-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY, amount numeric, note text);
CREATE PUBLICATION orders_filtered FOR TABLE app.orders (id, amount) WHERE (id > 1000);
