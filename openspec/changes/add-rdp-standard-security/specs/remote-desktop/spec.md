# Spec Delta

## ADDED Requirements

### Requirement: Legacy Standard RDP Security (opt-in)
RDP sessions SHALL offer a per-session option "Allow legacy RDP security", off by default. With it on, when the server refuses TLS/NLA because it only allows Standard RDP Security, the client SHALL reconnect using that layer (RSA key exchange, RC4 with 40/56/128-bit keys, MAC-checked PDUs) on Linux and Windows. It SHALL NOT fall back for any other negotiation result, nor for a host:port that already has a trusted TLS certificate.

#### Scenario: Server allows only Standard RDP Security, option on
- **WHEN** the option is on and the server's Security Layer is RDP
- **THEN** the session connects with Standard RDP Security, the desktop renders, and mouse, keyboard and resize work

#### Scenario: Option off
- **WHEN** the option is off and the server only allows Standard RDP Security
- **THEN** the connection fails before any credential is sent, with an error naming the option and the server-side fix (Security Layer Negotiate or SSL)

#### Scenario: No downgrade from a TLS-capable server
- **WHEN** the option is on and the server accepts TLS or NLA (untampered negotiation)
- **THEN** the session uses TLS or NLA as before and never Standard RDP Security

#### Scenario: Refusal from a host already trusted over TLS
- **WHEN** the option is on, the host:port has a trusted TLS certificate, and the X.224 answer is a Standard-RDP-only refusal (genuine or forged by an on-path attacker)
- **THEN** no legacy connection is attempted and the error says the host previously used TLS and the refusal may be a downgrade attempt

#### Scenario: Tampered or malformed traffic
- **WHEN** a server PDU fails the MAC check or its security header is malformed
- **THEN** the session ends with a protocol error and no further input is sent

### Requirement: Trust of the legacy server key
With Standard RDP Security, the client SHALL show the server's RSA public key fingerprint in a trust prompt that states the connection is not protected against interception, before any credential is sent. The key SHALL be saved and checked like a TLS certificate: a changed key stops the connection before credentials are sent.

#### Scenario: Unknown key
- **WHEN** the server presents a key not yet trusted
- **THEN** a prompt shows its fingerprint and the insecure-connection warning; cancelling closes the connection without sending credentials

#### Scenario: Changed key
- **WHEN** the server key differs from the trusted one
- **THEN** the connection stops before credentials are sent and both fingerprints are shown

### Requirement: Refused legacy encryption levels
The client SHALL refuse Standard RDP Security servers whose encryption level is None (credentials would travel in clear) or FIPS, with an error naming the level.

#### Scenario: Encryption level None
- **WHEN** the server selects encryption level None
- **THEN** the connection fails before credentials are sent and the error says the server offers no encryption

#### Scenario: FIPS encryption
- **WHEN** the server selects FIPS encryption
- **THEN** the connection fails with an error saying FIPS legacy encryption is not supported
