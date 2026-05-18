-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.sales (
  id bigint NOT NULL,
  region text NOT NULL,
  amount numeric NOT NULL,
  CONSTRAINT sales_pkey PRIMARY KEY (id)
);
