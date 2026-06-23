-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.l (a int);
CREATE TABLE app.r (b int);
CREATE TABLE app.c (LIKE app.l, mid int, LIKE app.r);
