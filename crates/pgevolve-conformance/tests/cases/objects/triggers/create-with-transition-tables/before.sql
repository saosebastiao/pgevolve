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
