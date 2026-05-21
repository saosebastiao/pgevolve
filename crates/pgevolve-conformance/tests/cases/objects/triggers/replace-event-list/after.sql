-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.products (
  id    bigint NOT NULL,
  price numeric NOT NULL,
  CONSTRAINT products_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.notify_change() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
CREATE TRIGGER trg_products_notify
  AFTER INSERT OR UPDATE ON app.products
  FOR EACH ROW
  EXECUTE FUNCTION app.notify_change();
