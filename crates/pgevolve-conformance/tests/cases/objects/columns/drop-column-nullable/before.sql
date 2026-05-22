-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.users (
    id    bigint NOT NULL,
    email text,
    notes text,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);
