-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.audit() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN END $$;
CREATE EVENT TRIGGER et_audit ON ddl_command_end EXECUTE FUNCTION app.audit();
ALTER EVENT TRIGGER et_audit ENABLE ALWAYS;
