#![allow(clippy::unwrap_used, clippy::expect_used)]

use ironrdp::pdu::rdp::server_license::cert::{ProprietaryCertificate, RsaPublicKey};

use super::*;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn randoms() -> ([u8; 32], [u8; 32]) {
    let mut client = [0u8; 32];
    let mut server = [0u8; 32];
    for ((c, s), i) in client.iter_mut().zip(server.iter_mut()).zip(0u8..) {
        *c = i;
        *s = 0x80 + i;
    }
    (client, server)
}

const DATA: [u8; 16] = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf];

/// Values printed by `tests/oracle/freerdp_vectors.c` (FreeRDP 3.10.3 algorithms):
/// bits, mac key, encrypt key, decrypt key, MAC, salted MAC (count 7), ciphertext of packets
/// 0, 4095, 4096 (first after the key update) and 4097.
const VECTORS: [(Bits, [&str; 9]); 3] = [
    (
        Bits::B40,
        [
            "d1269e9347180b6a",
            "d1269ecb8ddc9011",
            "d1269e59314b292d",
            "275924ad7d2abe43",
            "f66d6a05a00705bb",
            "e767897b8b83bdfaf04303c71fdbcbec",
            "bc9be65d451398a081545b1e327742dd",
            "c65ce67cef3070978bffa3c88816b2b9",
            "3dcf52b5bfc5a8b739b48fd44f68f04d",
        ],
    ),
    (
        Bits::B56,
        [
            "d1a9be9347180b6a",
            "d18177cb8ddc9011",
            "d174ee59314b292d",
            "274cd5edb7595ab0",
            "da74414acfecb388",
            "0bb8b448cc5b7cb457bcbb1cc907697c",
            "6cadb14168912ee9ab2475f76fb49a48",
            "6db47eb16cbfff073c495388c0f1c296",
            "8f2657a4c0ec5012883310a66500cb4e",
        ],
    ),
    (
        Bits::B128,
        [
            "67a9be9347180b6a49918713db69f2c5",
            "818177cb8ddc9011780552cb3d140abb",
            "4574ee59314b292dc2fbd69d00aa7331",
            "f6d3866465dac408",
            "a744b8e56351ce52",
            "1ffa6326e663ff64cd2f74ee6b86ce27",
            "57fbf70e4c7d719e73ae07cada6f6e13",
            "08b1c2baeae502f22f061895faa409ab",
            "80fda948914b77571460217e5561d73f",
        ],
    ),
];

#[test]
fn keys_macs_and_key_update_match_freerdp() {
    let (client, server) = randoms();
    for (bits, v) in VECTORS {
        let len = bits.key_len();
        let (mac_key, encrypt, decrypt) = session_keys(&client, &server, bits);
        assert_eq!(mac_key[..len], unhex(v[0])[..], "{bits:?} mac key");
        assert_eq!(encrypt[..len], unhex(v[1])[..], "{bits:?} encrypt key");
        assert_eq!(decrypt[..len], unhex(v[2])[..], "{bits:?} decrypt key");
        assert_eq!(mac(&mac_key[..len], &DATA, None)[..], unhex(v[3])[..], "{bits:?} MAC");
        assert_eq!(mac(&mac_key[..len], &DATA, Some(7))[..], unhex(v[4])[..], "{bits:?} salted MAC");

        let mut cipher = Cipher::new(encrypt, bits);
        for packet in 0..4098 {
            let mut buf = DATA;
            assert_eq!(cipher.apply(&mut buf, bits), packet);
            let want = match packet {
                0 => v[5],
                4095 => v[6],
                4096 => v[7],
                4097 => v[8],
                _ => continue,
            };
            assert_eq!(buf[..], unhex(want)[..], "{bits:?} packet {packet}");
        }
    }
}

const MODULUS: &str = "C3C4E2C2B1B9C5F2D3E1C0B5A4F7E6D9C8B7A6F5E4D3C2B1A0F9E8D7C6B5A49\
                       384736251403F2E1D0C1B2A3948576A5B4C3D2E1F0A9B8C7D6E5F4031425364758697A8B9";
const RSA: &str = "2bff714d9555fcc63bf5007666be307f445b5bbe6ac05d3340a81112d8559cb0084858a7344f5485f972d9b8df2d6f0a66\
                   088aef3177e7b8e54e02cb0bc7e54dec4c8794";

fn test_key() -> ServerKey {
    ServerKey { modulus: trim(&unhex(MODULUS)).to_vec(), exponent: vec![1, 0, 1] }
}

#[test]
fn rsa_matches_freerdp_with_trailing_zeros() {
    let (client, _) = randoms();
    let key = test_key();
    let out = key.encrypt(&client);
    let mut want = unhex(RSA);
    want.resize(key.modulus.len(), 0);
    want.extend([0; 8]);
    assert_eq!(out, want);
}

fn proprietary(modulus_be: &[u8]) -> Vec<u8> {
    let mut modulus: Vec<u8> = modulus_be.iter().rev().copied().collect();
    modulus.extend([0; 8]);
    let cert = ServerCertificate {
        issued_permanently: false,
        certificate: CertificateType::Proprietary(ProprietaryCertificate {
            public_key: RsaPublicKey { public_exponent: 65537, modulus },
            signature: vec![0; 72],
        }),
    };
    encode_vec(&cert).unwrap()
}

#[test]
fn proprietary_certificate_key_and_fingerprint() {
    let key = ServerKey::parse(&proprietary(&test_key().modulus)).unwrap();
    assert_eq!(key, test_key());
    let fp = key.fingerprint();
    assert!(fp.starts_with(KEY_PREFIX) && fp.len() == KEY_PREFIX.len() + 95, "{fp}");
}

#[test]
fn oversized_and_garbage_keys_are_rejected() {
    let big = vec![0xffu8; 513];
    assert!(ServerKey::parse(&proprietary(&big)).is_err());
    assert!(ServerKey::parse(&[9, 0, 0, 0, 1, 2]).is_err());
}

fn security_data(method: EncryptionMethod, level: EncryptionLevel) -> ServerSecurityData {
    ServerSecurityData {
        encryption_method: method,
        encryption_level: level,
        server_random: Some(randoms().1),
        server_cert: Vec::new(),
    }
}

#[test]
fn none_and_fips_are_refused() {
    let none = negotiated(&ServerSecurityData::no_security()).err().unwrap().to_string();
    assert!(none.contains("no encryption"), "{none}");
    let fips = negotiated(&security_data(EncryptionMethod::FIPS, EncryptionLevel::Fips)).err().unwrap().to_string();
    assert!(fips.contains("FIPS"), "{fips}");
    let (bits, _) = negotiated(&security_data(EncryptionMethod::BIT_128, EncryptionLevel::ClientCompatible)).unwrap();
    assert_eq!(bits, Bits::B128);
    let mut missing = security_data(EncryptionMethod::BIT_56, EncryptionLevel::High);
    missing.server_random = None;
    assert!(negotiated(&missing).is_err());
}

/// The client's state and the server's mirror (its encrypt key is the client's decrypt key).
fn pair(level: EncryptionLevel) -> (Security, Security) {
    let (client, server) = randoms();
    let c = Security::new(Bits::B128, level, &client, &server);
    let (mac_key, encrypt, decrypt) = session_keys(&client, &server, Bits::B128);
    let s = Security {
        bits: Bits::B128,
        level,
        mac_key,
        encrypt: Cipher::new(decrypt, Bits::B128),
        decrypt: Cipher::new(encrypt, Bits::B128),
        salted_seen: false,
    };
    (c, s)
}

fn request(user_data: &[u8]) -> Vec<u8> {
    let pdu = SendDataRequest { initiator_id: 1007, channel_id: 1003, user_data: Cow::Owned(user_data.to_vec()) };
    encode_vec(&X224(pdu)).unwrap()
}

fn indication(user_data: &[u8]) -> Vec<u8> {
    let pdu = SendDataIndication { initiator_id: 1002, channel_id: 1003, user_data: Cow::Owned(user_data.to_vec()) };
    encode_vec(&X224(pdu)).unwrap()
}

fn user_data(frame: &[u8]) -> Vec<u8> {
    match decode::<X224<McsMessage<'_>>>(frame).unwrap().0 {
        McsMessage::SendDataRequest(r) => r.user_data.into_owned(),
        McsMessage::SendDataIndication(i) => i.user_data.into_owned(),
        m => panic!("{m:?}"),
    }
}

/// What the server does with a sealed slow-path PDU: flags and plaintext.
fn server_open(server: &mut Security, frame: &[u8]) -> (u16, Vec<u8>) {
    let data = user_data(frame);
    let flags = u16::from_le_bytes([data[0], data[1]]);
    let mut body = data[12..].to_vec();
    server.open_body(&data[4..12], &mut body, false).unwrap();
    (flags, body)
}

/// What the server sends: an encrypted indication with `flags`.
fn server_seal(server: &mut Security, flags: u16, body: &[u8]) -> Vec<u8> {
    let mut body = body.to_vec();
    let sig = server.seal_body(&mut body);
    indication(&[&(flags | SEC_ENCRYPT).to_le_bytes()[..], &[0, 0], &sig, &body].concat())
}

#[test]
fn slow_path_round_trip_keeps_existing_headers() {
    let (mut c, mut s) = pair(EncryptionLevel::ClientCompatible);
    // Share Data (no header) and two PDUs in one buffer.
    let both = [request(b"share-data"), request(b"more")].concat();
    let sealed = c.seal(&both, false).unwrap();
    let first = frame_len(&sealed).unwrap();
    assert_eq!(server_open(&mut s, &sealed[..first]), (SEC_ENCRYPT, b"share-data".to_vec()));
    assert_eq!(server_open(&mut s, &sealed[first..]), (SEC_ENCRYPT, b"more".to_vec()));
    // Client Info keeps SEC_INFO_PKT; licensing adds SEC_LICENSE_ENCRYPT.
    let info = c.seal(&request(&[0x40, 0, 0, 0, 1, 2, 3]), true).unwrap();
    assert_eq!(server_open(&mut s, &info), (0x40 | SEC_ENCRYPT, vec![1, 2, 3]));
    let license = c.seal(&request(&[0x80, 0, 0, 0, 9]), true).unwrap();
    assert_eq!(server_open(&mut s, &license), (0x80 | SEC_ENCRYPT | SEC_LICENSE_ENCRYPT, vec![9]));

    // Inbound: data stripped to the bare body, licensing to a plain basic header.
    let opened = c.open(&server_seal(&mut s, 0, b"demand-active")).unwrap();
    assert_eq!(user_data(&opened), b"demand-active");
    let opened = c.open(&server_seal(&mut s, SEC_LICENSE_PKT | SEC_LICENSE_ENCRYPT, b"lic")).unwrap();
    assert_eq!(user_data(&opened), [0x80, 0, 0, 0, b'l', b'i', b'c']);
}

#[test]
fn salted_inbound_mac_uses_the_packet_count() {
    let (mut c, mut s) = pair(EncryptionLevel::High);
    for i in 0..3u8 {
        let mut body = vec![i; 5];
        let sig = mac(s.mac_key(), &body, Some(u32::from(i)));
        s.encrypt.apply(&mut body, Bits::B128);
        let frame =
            indication(&[&(SEC_ENCRYPT | SEC_SECURE_CHECKSUM).to_le_bytes()[..], &[0, 0], &sig, &body].concat());
        assert_eq!(user_data(&c.open(&frame).unwrap()), vec![i; 5]);
    }
}

#[test]
fn bad_signature_is_an_error() {
    let (mut c, mut s) = pair(EncryptionLevel::ClientCompatible);
    let mut frame = server_seal(&mut s, 0, b"payload");
    let last = frame.len() - 1;
    frame[last] ^= 1;
    let e = c.open(&frame).err().unwrap().to_string();
    assert!(e.contains("signature"), "{e}");
}

#[test]
fn clear_traffic_only_at_level_low() {
    let clear = indication(&[0, 0, 0, 0, b'x']);
    let (mut low, _) = pair(EncryptionLevel::Low);
    assert_eq!(user_data(&low.open(&clear).unwrap()), b"x");
    let fast = fp_frame(0, b"bitmap").unwrap();
    assert_eq!(low.open(&fast).unwrap(), fast);
    let (mut high, _) = pair(EncryptionLevel::ClientCompatible);
    assert!(high.open(&clear).is_err());
    assert!(high.open(&fast).is_err());
    // Licensing PDUs may arrive in clear at any level.
    let license = indication(&[0x80, 0, 0, 0, b'l']);
    assert_eq!(user_data(&high.open(&license).unwrap()), [0x80, 0, 0, 0, b'l']);
}

#[test]
fn fast_path_round_trip_both_directions() {
    let (mut c, mut s) = pair(EncryptionLevel::ClientCompatible);
    // Input with 2 events, and a long one that needs a 2-byte length.
    for events in [vec![1u8, 2, 3, 4], vec![7u8; 300]] {
        let plain = fp_frame(2 << 2, &events).unwrap();
        let sealed = c.seal(&plain, false).unwrap();
        assert_eq!(sealed[0] >> 6, FASTPATH_ENCRYPTED);
        assert_eq!(sealed[0] & 0x3f, 2 << 2);
        let (len, n) = fp_length(&sealed).unwrap();
        assert_eq!(len, sealed.len());
        let mut body = sealed[1 + n + 8..].to_vec();
        s.open_body(&sealed[1 + n..1 + n + 8], &mut body, false).unwrap();
        assert_eq!(body, events);
    }
    // Output: server encrypts, client restores the plain frame.
    let update = vec![0x55u8; 200];
    let mut body = update.clone();
    let sig = s.seal_body(&mut body);
    let frame = fp_frame(FASTPATH_ENCRYPTED << 6, &[&sig[..], &body].concat()).unwrap();
    assert_eq!(c.open(&frame).unwrap(), fp_frame(0, &update).unwrap());
}

#[test]
fn exchange_pdu_carries_the_encrypted_random() {
    let data = security_data(EncryptionMethod::BIT_128, EncryptionLevel::ClientCompatible);
    let ex = exchange(&data, &test_key(), 1007, 1003).unwrap();
    let ud = user_data(&ex.pdu);
    assert_eq!(ud[..4], [0x01, 0x02, 0, 0]);
    let len = usize::try_from(u32::from_le_bytes([ud[4], ud[5], ud[6], ud[7]])).unwrap();
    assert_eq!(len, test_key().modulus.len() + 8);
    assert_eq!(ud.len(), 8 + len);
    assert!(ud[ud.len() - 8..].iter().all(|b| *b == 0));
}
