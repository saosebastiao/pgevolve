-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
  id bigint NOT NULL,
  price numeric,
  CONSTRAINT products_pkey PRIMARY KEY (id)
);
