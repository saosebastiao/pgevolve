-- Pre-create roles referenced by grants in this fixture.
-- setup.sql is executed before before.sql and is not parsed by pgevolve.
CREATE ROLE app_owner;
CREATE ROLE readers;
