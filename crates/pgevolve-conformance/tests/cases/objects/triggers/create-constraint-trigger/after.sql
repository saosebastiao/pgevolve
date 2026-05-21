-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.items (
  id      bigint NOT NULL,
  status  text   NOT NULL,
  CONSTRAINT items_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.check_item_status() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  IF NEW.status NOT IN ('active', 'archived') THEN
    RAISE EXCEPTION 'invalid status: %', NEW.status;
  END IF;
  RETURN NEW;
END
$$;
CREATE CONSTRAINT TRIGGER trg_items_status_check
  AFTER INSERT OR UPDATE ON app.items
  DEFERRABLE INITIALLY DEFERRED
  FOR EACH ROW
  EXECUTE FUNCTION app.check_item_status();
