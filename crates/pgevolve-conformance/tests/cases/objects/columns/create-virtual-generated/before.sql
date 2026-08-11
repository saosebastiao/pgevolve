-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.items (
    id  bigint NOT NULL,
    qty integer NOT NULL,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
