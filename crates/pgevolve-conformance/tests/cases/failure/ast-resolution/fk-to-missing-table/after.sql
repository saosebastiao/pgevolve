-- @pgevolve schema=app
CREATE SCHEMA app;
CREATE TABLE app.orders (
    id bigint PRIMARY KEY,
    user_id bigint NOT NULL,
    CONSTRAINT fk_user FOREIGN KEY (user_id) REFERENCES app.users (id)
);
