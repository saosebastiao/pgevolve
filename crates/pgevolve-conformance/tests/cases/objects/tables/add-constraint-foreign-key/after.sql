-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.categories (
    id   bigint NOT NULL,
    name text NOT NULL,
    CONSTRAINT categories_pkey PRIMARY KEY (id)
);
CREATE TABLE app.products (
    id          bigint NOT NULL,
    name        text NOT NULL,
    category_id bigint,
    CONSTRAINT products_pkey PRIMARY KEY (id),
    CONSTRAINT products_category_id_fkey FOREIGN KEY (category_id) REFERENCES app.categories (id)
);
