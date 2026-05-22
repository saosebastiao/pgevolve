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
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
