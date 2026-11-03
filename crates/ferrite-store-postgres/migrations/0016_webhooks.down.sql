-- Reverse of 0016_webhooks.up.sql. Not declared lossy, like 0011 to 0013: the delivery
-- log is transient (30 days), and the endpoints are configuration an administrator
-- registers again; no content is removed.
DROP TABLE webhook_attempts;
DROP TABLE webhook_deliveries;
DROP TABLE webhook_events;
DROP TABLE webhook_endpoints;
