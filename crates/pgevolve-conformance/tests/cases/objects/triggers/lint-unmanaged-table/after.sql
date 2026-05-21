-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.on_ghost_insert() RETURNS trigger
    LANGUAGE plpgsql
AS $$
BEGIN
  RETURN NEW;
END
$$;
-- trigger fires on app.ghost_table which is not declared in this project;
-- the trigger-references-unmanaged-table lint (Error severity) fires at CLI
-- plan time. The in-process planner still generates the CREATE TRIGGER step.
CREATE TRIGGER trg_ghost_insert
  AFTER INSERT ON app.ghost_table
  FOR EACH ROW
  EXECUTE FUNCTION app.on_ghost_insert();
