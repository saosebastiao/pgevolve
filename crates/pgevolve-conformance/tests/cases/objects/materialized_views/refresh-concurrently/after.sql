-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.sales (
  id bigint NOT NULL,
  region text NOT NULL,
  amount numeric NOT NULL,
  CONSTRAINT sales_pkey PRIMARY KEY (id)
);
CREATE MATERIALIZED VIEW app.revenue_summary AS
  SELECT region, sum(amount) AS total
  FROM app.sales
  GROUP BY region;
CREATE UNIQUE INDEX revenue_summary_region_uidx
  ON app.revenue_summary (region);
