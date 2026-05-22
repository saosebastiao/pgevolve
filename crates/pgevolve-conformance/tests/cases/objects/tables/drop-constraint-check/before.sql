-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
    id    bigint NOT NULL,
    price numeric NOT NULL,
    CONSTRAINT products_pkey PRIMARY KEY (id),
    CONSTRAINT products_price_positive CHECK (price > 0)
);
