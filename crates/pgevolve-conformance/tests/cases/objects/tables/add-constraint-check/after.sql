-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
    id       bigint NOT NULL,
    quantity integer NOT NULL,
    CONSTRAINT products_pkey PRIMARY KEY (id),
    CONSTRAINT products_quantity_positive CHECK (quantity > 0)
);
