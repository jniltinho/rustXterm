# Tasks

## 1. Option and actionable error

- [x] 1.1 `connector_err` maps `SSL_NOT_ALLOWED_BY_SERVER` to `STANDARD_ONLY`, naming "Allow legacy RDP security" and the server fix; `hostile_rdp::standard_rdp_security_only_gives_actionable_error` asserts both
- [x] 1.2 `RdpOptions::allow_standard_security` (serde default `false`) in `crates/core/src/session.rs`; `rdp_legacy_tls_is_opt_in_and_round_trips` covers old configs and the round trip; `crates/store/tests/export.rs` round-trips it
- [x] 1.3 Session editor checkbox "Allow legacy RDP security (no TLS, insecure)" next to "Allow legacy TLS"; `snapshot_m2_editors_and_certificate` renders it ticked in light and dark (`editor_rdp_options_*`)

## 2. Vendored IronRDP patches

- [x] 2.1 Vendor `ironrdp-connector` 0.10.0 and `ironrdp-pdu` 0.9.0 under `vendor/` with `[patch.crates-io]` entries and a `vendor/README.md` section (why, exact change, removal criteria); every change carries a `rustXterm:` comment
- [x] 2.2 `ironrdp-connector`: neither TLS nor CredSSP requested means `PROTOCOL_RDP` (accepted as selected), 40/56/128-bit methods (never FIPS), no auto-detect flag and no message channel in that mode, `server_security` kept; TLS/NLA path unchanged (`lab_rdp`, `legacy_tls`, `hostile_rdp` and the VM NLA tests green)
- [x] 2.3 `ironrdp-pdu` `MAX_SERVER_CERT_LEN` to 16 KiB (`server_security_data_accepts_large_certificate_chains`: 3 KiB decodes, 20 KiB rejected); licensing fills a Server License Request without certificate from the GCC certificate (see 8.1 for its test)

## 3. Legacy security crypto (`crates/rdp/src/legacy.rs`)

- [x] 3.1 Test-only oracle `crates/rdp/tests/oracle/freerdp_vectors.c` (FreeRDP 3.10.3 algorithms on OpenSSL, built in an Alpine container, not shipped) prints keys, MACs, ciphertexts across the 4096-packet update and an RSA vector; output identical across runs
- [x] 3.2 RC4, key derivation, MAC and salted MAC, key update; `keys_macs_and_key_update_match_freerdp` (40/56/128-bit) and `salted_inbound_mac_uses_the_packet_count`
- [x] 3.3 Server certificate parsing (proprietary blob; X.509 chain leaf) and RSA of the client random (little-endian, modulus width plus 8 zero bytes); `rsa_matches_freerdp_with_trailing_zeros`, `proprietary_certificate_key_and_fingerprint`, `oversized_and_garbage_keys_are_rejected`
- [x] 3.4 Encryption level None and FIPS refused with the spec's errors; `none_and_fips_are_refused`

## 4. Wire transform and negotiation fallback

- [x] 4.1 `Wire` runs the D2 transform per PDU kind (Client Info and licensing headers kept, data headers added/stripped, fast-path flags and signature, every inbound PDU decrypted even when dropped), clear inbound traffic only at level Low; `slow_path_round_trip_keeps_existing_headers`, `fast_path_round_trip_both_directions`, `clear_traffic_only_at_level_low`, `bad_signature_is_an_error`, `exchange_pdu_carries_the_encrypted_random`
- [x] 4.2 Licensing per D2b: Security Exchange advertises `SEC_LICENSE_ENCRYPT_SC`; client license PDUs in clear until the server marks its licensing PDUs `SEC_LICENSE_ENCRYPT_CS`, then encrypted; inbound license PDUs decrypted when flagged and accepted in clear; proprietary licensing key fixed to big-endian (`licensing_public_key_of_a_proprietary_certificate_is_big_endian`); covered by `slow_path_round_trip_keeps_existing_headers` and the Low-level tests (full handshake: 8.1)
- [x] 4.4 Review fixes: length-prefixed key fingerprint and RSA parameter checks (odd exponent ≥ 3, odd modulus; `fingerprint_binds_the_modulus_exponent_split`); zero-length and truncated frames are errors, never panics (`empty_and_truncated_frames_are_errors`, `Wire::take`)
- [x] 4.3 `establish` retries with `PROTOCOL_RDP` only on `SSL_NOT_ALLOWED_BY_SERVER`, with the option on and no saved TLS certificate for host:port; `standard_security_refusal_reconnects_requesting_protocol_rdp`, `other_negotiation_failures_never_fall_back`, `forged_refusal_for_a_tls_trusted_host_is_a_downgrade_error`

## 5. Trust prompt for the server key

- [x] 5.1 `RSA `-prefixed SHA-256 fingerprint of the server key through `CertTrust` and the shared `decide_then_password` (prompt before the Security Exchange and before any credential; a changed key is refused like a certificate); exercised by the VM test (`CertificateUnknown` with the `RSA ` fingerprint, then the password)
- [x] 5.2 The prompt subject carries the interception warning (CLI/MCP unchanged); the UI titles it "Unknown server key" with the warning in the danger color; `dialog_legacy_rdp_key_unknown_{light,dark}` snapshots

## 6. Docs and receipts

- [x] 6.1 User guide `docs/rdp.md` (options, security layers, legacy mode trade-offs, downgrade guard, server-side fix, errors table), linked from `docs/README.md`; VM recipe for switching layers in `lab/vagrant/README.md`
- [x] 6.2 `vm_rdp` tests `rdp_windows_standard_security_login_input_and_resize` and `rdp_windows_standard_security_off_is_actionable`; receipt `docs/gates/rdp-standard-security.md` (Linux and Windows clients, levels Low/Client Compatible/High, NLA regression)

## 7. Verification

- [x] 7.1 fmt, clippy (`--workspace --all-targets -D warnings`, Linux and Windows targets) and workspace tests green in Docker; `cargo xtask guard` and `cargo xtask layers` pass; Windows cross build of the RDP tests runs on the VM

## 8. Follow-up (not needed for the test build)

- [ ] 8.1 Full licensing handshake test with a Server License Request that omits its certificate (in-process server), and a receipt against a server with the RD Licensing role
- [ ] 8.2 In-process Standard-Security server in `crates/rdp/tests/legacy_rdp.rs` reaching the active stage (today covered by the Windows VM receipt and the transform unit tests)
- [ ] 8.3 Receipt against a server that sends an X.509 chain in Server Security Data
- [ ] 8.4 Report the encryption method, level and salted-MAC use in the session log of the app
- [ ] 8.5 "Forget certificate" for a host:port (UI and CLI), so the downgrade guard can be cleared without renaming the host

## Workflow follow-up

- Archive the change with `/opsx:archive` once group 8 is done or split into its own change.
