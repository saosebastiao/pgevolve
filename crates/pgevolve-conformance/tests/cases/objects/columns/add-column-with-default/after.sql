-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
    id       bigint NOT NULL,
    name     text NOT NULL,
    quantity integer DEFAULT 0,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
