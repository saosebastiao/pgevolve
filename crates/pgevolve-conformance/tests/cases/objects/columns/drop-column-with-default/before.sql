-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
    id       bigint NOT NULL,
    status   text DEFAULT 'pending',
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
