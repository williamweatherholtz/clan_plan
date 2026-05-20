-- Patch _sqlx_migrations checksums to match LF-form migration files.
-- Generated 2026-05-01T22:51:54Z for git sha b78f965
-- These hashes are sha384 of each migrations/*.sql file with LF endings.
-- Safe iff the only delta is line endings; verify schema state if unsure.

BEGIN;

-- 001_initial.sql
UPDATE _sqlx_migrations SET checksum = decode('06c8e535808716c67f5dda63aabfcfdc2ffad61bf473718384bf813a186a6bbdf6f8ffaf5ae9632fefff1d20ff3254bf', 'hex') WHERE version = 1;
-- 002_reunion_slug.sql
UPDATE _sqlx_migrations SET checksum = decode('1d15a2a96462f8fa1d27b9e38724f09d35c29c450caf4e3f2740b3ddb9b2478d34910dfa408fff04a13f17dcb902517b', 'hex') WHERE version = 2;
-- 003_avail_poll_window.sql
UPDATE _sqlx_migrations SET checksum = decode('863205cbd6be3187434de8214038681178f989a1346178bb96d692890cc3041e6a794c88058bbd413e922778c368b8a2', 'hex') WHERE version = 3;
-- 004_expense_confirmations.sql
UPDATE _sqlx_migrations SET checksum = decode('8e85a165fec9cc452f0c1b26fdf9f79716b4117361d228df093ff4043fe2e7d0ee65aff6bd80875b126b868db8a4e9f0', 'hex') WHERE version = 4;
-- 005_remove_travel_block_type.sql
UPDATE _sqlx_migrations SET checksum = decode('aaf6100a73ee47d11b6fc8abd8536a11a51552e2bc35c3682f1b0088d2d3e24ccbfd6261c298fc5ee70c9c8dc822df10', 'hex') WHERE version = 5;
-- 006_activity_rsvps.sql
UPDATE _sqlx_migrations SET checksum = decode('12feb3d4fa670084a996ae36518d951ba20ba418ff54322bfbadc06ea5c9f5053168eee54487c37620aedc2c8d4aae0d', 'hex') WHERE version = 6;
-- 007_reunion_family_units.sql
UPDATE _sqlx_migrations SET checksum = decode('6cba6bdb7faeb826f8decb9d9dd0ae18d461742e2363f580d434720c8a6624f61627b621b1642663cdb4a63e1c26807b', 'hex') WHERE version = 7;
-- 008_survey_multi_response.sql
UPDATE _sqlx_migrations SET checksum = decode('b0f7a368233d9cc15fe2e8c44a3ec8f55c2bb6db189c705ad5b9547d6ff873af3c5652b4a8df2b4e65a4bd3566c005e1', 'hex') WHERE version = 8;
-- 009_reunion_admins.sql
UPDATE _sqlx_migrations SET checksum = decode('3b782c94e26fd26a3fbf3ea5b4ea66ec9af287cd6793dd8e579640cbd71e152bd748bf0b9dfa7ec64e452d2e0a1d7616', 'hex') WHERE version = 9;
-- 010_simplify_phases.sql
UPDATE _sqlx_migrations SET checksum = decode('f573bc0661c114ba81ac593b6c33aefbae70bab29cb3540da57134d2947ec29e0f0ec1f468b69733e3aced1c0e2aedcd', 'hex') WHERE version = 10;
-- 011_prep_completed_and_location_tz.sql
UPDATE _sqlx_migrations SET checksum = decode('4cbd59d7199ae00c2053bb9633e40576345ecfcf0fcd1a328f05c1abbd8f4a225e660b53f7ec1db30b7408b41c06a859', 'hex') WHERE version = 11;
-- 012_remove_schedule_phase.sql
UPDATE _sqlx_migrations SET checksum = decode('66c6024eb183bcbd897c8f6b31cfda89ebc76411ba4565329cf39a4634aa7f95a89a39eb756c9e3ff59836d6759c8576', 'hex') WHERE version = 12;
-- 013_default_activity_duration.sql
UPDATE _sqlx_migrations SET checksum = decode('9dd2b51008043e7acf06815ecac88429c28f0a036940de78f63a48362b28cbe2f97f142543f8330c308e332f36b96f84', 'hex') WHERE version = 13;
-- 014_registration_setting.sql
UPDATE _sqlx_migrations SET checksum = decode('f9090441cf614169f63361e14d104561e3667f95efd97998789bcef8636255c84eca294835435bb76cf76766f58993c6', 'hex') WHERE version = 14;
-- 015_login_attempts.sql
UPDATE _sqlx_migrations SET checksum = decode('835eba37bd1bb74918471889cf0bfcd3385208291d76a3e02acd5d836264cfe0145f5c3ee8ae1bac0d682d0cf469d356', 'hex') WHERE version = 15;
-- 016_activity_category.sql
UPDATE _sqlx_migrations SET checksum = decode('1faf964ca9ed1b33758571dd8b4a3ad50fc380b1cb56a10366f5ec2d480de3f13b170f992c25a27c4056a021f9a50803', 'hex') WHERE version = 16;
-- 017_reunion_invites.sql
UPDATE _sqlx_migrations SET checksum = decode('5fb7e8f67cf761a87c2e88346a5ef68be462cf2f3308fc52970e15be66824fd75f68b26300f340b4f74221d8dfec8572', 'hex') WHERE version = 17;

-- Show before-commit verification (rows with their new checksums):
SELECT version, description, length(checksum) AS sum_len FROM _sqlx_migrations ORDER BY version;

COMMIT;
