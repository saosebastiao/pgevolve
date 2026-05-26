-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (id bigint PRIMARY KEY, status text, total numeric);
