-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE FUNCTION app.audit() RETURNS event_trigger LANGUAGE plpgsql AS $$ BEGIN END $$;
CREATE EVENT TRIGGER et_audit ON ddl_command_end EXECUTE FUNCTION app.audit();
COMMENT ON EVENT TRIGGER et_audit IS 'audits DDL';
