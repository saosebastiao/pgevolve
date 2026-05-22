-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.measurements (
    id     bigint NOT NULL,
    width  numeric NOT NULL,
    height numeric NOT NULL,
    CONSTRAINT measurements_pkey PRIMARY KEY (id)
);
