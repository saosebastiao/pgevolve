-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TYPE app.int_window AS RANGE (subtype = int4);
CREATE TABLE app.reservations (
  id bigint NOT NULL,
  span app.int_window NOT NULL,
  CONSTRAINT reservations_pkey PRIMARY KEY (id)
);
