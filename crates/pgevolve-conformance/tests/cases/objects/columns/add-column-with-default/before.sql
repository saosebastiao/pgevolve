-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
    id    bigint NOT NULL,
    name  text NOT NULL,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
