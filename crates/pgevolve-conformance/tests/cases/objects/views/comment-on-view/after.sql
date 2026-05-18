-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
  id bigint NOT NULL,
  name text,
  CONSTRAINT products_pkey PRIMARY KEY (id)
);
CREATE VIEW app.annotated_view (id, name) AS
  SELECT id, name FROM app.products;
COMMENT ON VIEW app.annotated_view IS 'Product catalogue view';
COMMENT ON COLUMN app.annotated_view.name IS 'Product display name';
