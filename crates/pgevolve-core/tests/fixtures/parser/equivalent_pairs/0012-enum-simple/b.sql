-- @pgevolve schema=app
CREATE TYPE order_status AS ENUM ('pending', 'shipped', 'delivered');
