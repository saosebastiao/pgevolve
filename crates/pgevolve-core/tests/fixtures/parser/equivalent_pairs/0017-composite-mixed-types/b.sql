-- @pgevolve schema=app
CREATE TYPE line_item AS (
    sku text,
    qty integer,
    price numeric(10, 2)
);
