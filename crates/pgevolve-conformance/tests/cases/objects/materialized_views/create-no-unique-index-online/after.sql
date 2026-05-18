-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
  id bigint NOT NULL,
  amount numeric NOT NULL,
  CONSTRAINT orders_pkey PRIMARY KEY (id)
);
-- No unique index — refresh will be plain (not CONCURRENTLY) under online strategy
CREATE MATERIALIZED VIEW app.nolynx_report AS
  SELECT id, amount FROM app.orders;
