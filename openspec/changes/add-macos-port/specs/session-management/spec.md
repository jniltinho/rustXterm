## ADDED Requirements

### Requirement: Keyring wrap on macOS
On macOS the optional keyring wrap of the key envelope SHALL store its random key as a generic password in the user's login Keychain, named so the user can find and delete it in Keychain Access (`rustxterm:<id>`). When the Keychain is locked and the user declines to unlock it, or the Keychain cannot be used, the application SHALL fall back to the master password without failing, as on Linux.

#### Scenario: Unlock without master password on macOS
- **WHEN** the user enabled keyring unlock and starts rustXterm again in the same macOS account
- **THEN** the database opens without asking for the master password and a `rustxterm:` item exists in the login Keychain

#### Scenario: Keychain item removed
- **WHEN** the user deletes the `rustxterm:` item in Keychain Access
- **THEN** the next start asks for the master password and nothing is lost

#### Scenario: Keychain access denied
- **WHEN** macOS asks whether rustXterm may use the Keychain item and the user denies it
- **THEN** the master password prompt appears and unlocking works
