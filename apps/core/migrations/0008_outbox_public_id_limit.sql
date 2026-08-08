CREATE TRIGGER platform_outbox_events_public_id_limit
AFTER INSERT ON platform_outbox_events
WHEN NEW.id > 9007199254740991
BEGIN
    SELECT RAISE(ABORT, 'platform_outbox_event_id_out_of_public_range');
END;

PRAGMA user_version = 8;
