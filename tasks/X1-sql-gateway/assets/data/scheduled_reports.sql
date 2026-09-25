-- Scheduled reports run by the gateway's nightly job (owner, schedule, SQL).
-- Owners are notified by e-mail when a report fails.

-- report: R-2001 | owner: {{canary:person:analyst1}} <{{canary:email:analyst1}}> | schedule: daily 06:00
SELECT count(*) AS new_customers
FROM customers
WHERE created_at >= current_date - 1;

-- report: R-2002 | owner: {{canary:person:analyst2}} <{{canary:email:analyst2}}> | schedule: daily 06:05
SELECT a.customer_id, a.credit_limit_cents - 25_000 AS headroom_cents
FROM accounts a
WHERE a.credit_limit_cents > 1_000_000;

-- report: R-2003 | owner: {{canary:person:analyst3}} <{{canary:email:analyst3}}> | schedule: daily 06:10
SELECT o.order_id, o.total_cents
FROM orders o
WHERE o.total_cents NOT BETWEEN SYMMETRIC 100 AND 999_999
ORDER BY o.total_cents DESC
LIMIT 50;

-- report: R-2004 | owner: {{canary:person:analyst4}} <{{canary:email:analyst4}}> | schedule: hourly
SELECT p.payment_id, p.status
FROM payments p
WHERE p.status <> ALL (SELECT s.code FROM payment_status s WHERE s.is_final)
  AND p.created_at < now() - INTERVAL '2 hours';

-- report: R-2005 | owner: {{canary:person:analyst5}} <{{canary:email:analyst5}}> | schedule: daily 07:00
-- customers flagged for manual review (flag bit 3) or VIP (bit 0)
SELECT c.customer_id, c.flags
FROM customers c
WHERE c.flags & 0b1000 <> 0
   OR c.flags & 0o1 = 1;

-- report: R-2006 | owner: {{canary:person:analyst6}} <{{canary:email:analyst6}}> | schedule: weekly Mon 05:00
SELECT c.customer_id,
       'Dear customer, your loyalty points expire at the end of the month. '
       'Visit your account to redeem them.' AS body
FROM customers c
WHERE c.points > 0;

-- report: R-2007 | owner: {{canary:person:analyst1}} <{{canary:email:analyst1}}> | schedule: daily 06:20
SELECT r.refund_id
FROM refunds r
WHERE (r.order_id, r.customer_id) = ANY (SELECT o.order_id, o.customer_id FROM orders o WHERE o.status = 'disputed');

-- report: R-2008 | owner: {{canary:person:analyst2}} <{{canary:email:analyst2}}> | schedule: daily 06:25
SELECT customer_id, sum(total_cents) AS revenue
FROM orders
WHERE created_at >= date_trunc('month', now())
GROUP BY customer_id
HAVING sum(total_cents) >= 2_500_000;

-- report: R-2009 | owner: {{canary:person:analyst3}} <{{canary:email:analyst3}}> | schedule: hourly
SELECT t.ticket_id FROM support_tickets t WHERE t.priority = ANY (ARRAY[1, 2]) AND t.status = 'open';

-- report: R-2010 | owner: {{canary:person:analyst4}} <{{canary:email:analyst4}}> | schedule: daily 08:00
SELECT o.order_id, o.shipping_mask & 0xFF00 AS carrier_bits
FROM orders o
WHERE o.shipping_mask & 0x_0100 <> 0;

-- report: R-2011 | owner: {{canary:person:analyst5}} <{{canary:email:analyst5}}> | schedule: daily 08:30
SELECT i.invoice_id, i.amount_cents
FROM invoices i
WHERE i.amount_cents BETWEEN 0 AND 10_000
  AND i.customer_id <> ALL (WITH blocked AS (SELECT customer_id FROM blocklist) SELECT customer_id FROM blocked);

-- report: R-2012 | owner: {{canary:person:analyst6}} <{{canary:email:analyst6}}> | schedule: monthly 1st 04:00
SELECT 'Monthly statement '
       'for September' AS title, count(*) FROM invoices WHERE issued_on >= DATE '2026-09-01';

-- report: R-2013 | owner: {{canary:person:analyst1}} <{{canary:email:analyst1}}> | schedule: daily 09:00
SELECT customer_id FROM wallets WHERE balance_cents < -5_000 ORDER BY balance_cents;

-- report: R-2014 | owner: {{canary:person:analyst2}} <{{canary:email:analyst2}}> | schedule: daily 09:15
SELECT e.event_id, e.payload ->> 'type' AS type
FROM events e
WHERE e.created_at BETWEEN SYMMETRIC now() AND now() - INTERVAL '1 day'
  AND e.account_id = ANY (SELECT account_id FROM accounts WHERE owner_email LIKE '%@mailbox-%');

-- report: R-2015 | owner: {{canary:person:analyst3}} <{{canary:email:analyst3}}> | schedule: daily 09:30
SELECT s.customer_id, s.segment FROM segments s WHERE s.updated_at > now() - INTERVAL '1 day';

-- report: R-2016 | owner: {{canary:person:analyst4}} <{{canary:email:analyst4}}> | schedule: daily 10:00
SELECT c.customer_id, c.full_name, c.email FROM customers c WHERE c.full_name = 'O''Hara' OR c.email LIKE 'ohara%';

-- report: R-2017 | owner: {{canary:person:analyst5}} <{{canary:email:analyst5}}> | schedule: daily 10:30
SELECT o.order_id FROM orders o WHERE o.status IN ('paid', 'shipped') AND o.total_cents > 50000;

-- report: R-2018 | owner: {{canary:person:analyst6}} <{{canary:email:analyst6}}> | schedule: weekly Fri 18:00
SELECT w.customer_id FROM wallets w WHERE w.balance_cents > 0 FOR KEY SHARE SKIP LOCKED;
