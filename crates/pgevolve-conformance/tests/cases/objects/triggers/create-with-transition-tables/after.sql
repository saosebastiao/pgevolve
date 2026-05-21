-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.inventory (
  id       bigint  NOT NULL,
  quantity integer NOT NULL,
  CONSTRAINT inventory_pkey PRIMARY KEY (id)
);
CREATE FUNCTION app.sync_inventory_changes() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NULL;
END
$$;
CREATE TRIGGER trg_inventory_sync
  AFTER UPDATE ON app.inventory
  REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
  FOR EACH STATEMENT
  EXECUTE FUNCTION app.sync_inventory_changes();
