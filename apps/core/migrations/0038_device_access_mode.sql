-- Existing device tokens retain their prior administrator behavior; TV explicitly enrolls restricted.
ALTER TABLE identity_devices ADD COLUMN access_mode TEXT NOT NULL DEFAULT 'administrator'
    CHECK(access_mode IN ('administrator','media-read-only'));
-- Existing pairing codes preserve administrator compatibility; TV codes enforce a lower ceiling.
ALTER TABLE identity_pairing_codes ADD COLUMN access_mode TEXT NOT NULL DEFAULT 'administrator'
    CHECK(access_mode IN ('administrator','media-read-only'));
PRAGMA user_version = 38;
