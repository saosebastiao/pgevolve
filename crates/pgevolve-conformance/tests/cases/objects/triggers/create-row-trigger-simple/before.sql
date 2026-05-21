-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
  id      bigint NOT NULL,
  total   numeric NOT NULL,
  CONSTRAINT orders_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.stamp_audit() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
