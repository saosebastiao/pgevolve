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
