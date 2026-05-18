-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
  id bigint NOT NULL,
  category text NOT NULL,
  price numeric NOT NULL,
  CONSTRAINT products_pkey PRIMARY KEY (id)
);
CREATE MATERIALIZED VIEW app.product_summary AS
  SELECT category, count(*) AS cnt, avg(price) AS avg_price
  FROM app.products
  GROUP BY category;
CREATE INDEX product_summary_category_idx
  ON app.product_summary (category);
