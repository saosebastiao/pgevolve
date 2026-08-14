-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
    id     bigint NOT NULL,
    amount integer NOT NULL,
    CONSTRAINT orders_pkey PRIMARY KEY (id),
    CONSTRAINT orders_amount_positive CHECK (amount > 0) NOT ENFORCED
);
