-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
  id bigint NOT NULL,
  price numeric,
  CONSTRAINT products_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.tax_rate() RETURNS numeric
    LANGUAGE sql IMMUTABLE
AS $$ SELECT 0.1 $$;
CREATE VIEW app.products_with_tax AS
  SELECT id, price, price * app.tax_rate() AS tax
  FROM app.products;
