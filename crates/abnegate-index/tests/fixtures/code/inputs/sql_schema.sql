-- Orders schema for the widgets shop.

CREATE TABLE IF NOT EXISTS customers (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE orders (
    id UUID PRIMARY KEY,
    customer_id UUID NOT NULL REFERENCES customers (id) ON DELETE CASCADE,
    total_cents BIGINT NOT NULL CHECK (total_cents >= 0),
    status TEXT NOT NULL DEFAULT 'pending'
);

CREATE INDEX orders_customer_idx ON orders (customer_id);

CREATE OR REPLACE FUNCTION order_count(target UUID) RETURNS BIGINT AS $$
    SELECT count(*) FROM orders WHERE customer_id = target;
$$ LANGUAGE SQL STABLE;

ALTER TABLE orders ADD COLUMN shipped_at TIMESTAMPTZ;

INSERT INTO customers (id, email) VALUES ('00000000-0000-0000-0000-000000000001', 'ada@example.com');
