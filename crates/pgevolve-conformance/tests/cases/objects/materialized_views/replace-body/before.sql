-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
  id bigint NOT NULL,
  customer_id bigint NOT NULL,
  amount numeric NOT NULL,
  CONSTRAINT orders_pkey PRIMARY KEY (id)
);
CREATE MATERIALIZED VIEW app.order_stats AS
  SELECT customer_id, count(*) AS order_count
  FROM app.orders
  GROUP BY customer_id;
