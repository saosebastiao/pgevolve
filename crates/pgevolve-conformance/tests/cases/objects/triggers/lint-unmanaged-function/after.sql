-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.widgets (
  id   bigint NOT NULL,
  name text   NOT NULL,
  CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
-- trigger executes app.ghost_fn which is not declared in this project;
-- the trigger-references-unmanaged-function lint (Error severity) fires at CLI
-- plan time. The in-process planner still generates the CREATE TRIGGER step.
CREATE TRIGGER trg_widgets_hook
  AFTER INSERT ON app.widgets
  FOR EACH ROW
  EXECUTE FUNCTION app.ghost_fn();
