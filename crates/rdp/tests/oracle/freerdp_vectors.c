/*
 * Test-only oracle for crates/rdp/src/legacy/tests.rs (Standard RDP Security, MS-RDPBCGR 5.3).
 * The algorithms below are a line-for-line port of FreeRDP 3.10.3 libfreerdp/core/security.c and
 * crypto.c (Apache-2.0, Copyright 2011 Marc-Andre Moreau, 2014 Norbert Federa) onto OpenSSL, with
 * fixed inputs. It is never built into rustXterm; the printed values are frozen in the Rust tests.
 *
 * Run (Docker, no toolchain on the desktop):
 *   docker run --rm -v "$PWD/crates/rdp/tests/oracle:/o" -w /o alpine:latest sh -c \
 *     'apk add -q gcc musl-dev openssl-dev && gcc -O2 -o /tmp/v freerdp_vectors.c -lcrypto && /tmp/v'
 */
#include <openssl/bn.h>
#include <openssl/evp.h>
#include <stdio.h>
#include <string.h>

typedef unsigned char BYTE;

static const BYTE pad1[40] = { [0 ... 39] = 0x36 };
static const BYTE pad2[48] = { [0 ... 47] = 0x5C };
static const BYTE salt[] = { 0xD1, 0x26, 0x9E };

static void digest(const char* md, const BYTE* const* parts, const size_t* lens, int n, BYTE* out)
{
	EVP_MD_CTX* c = EVP_MD_CTX_new();
	EVP_DigestInit_ex(c, EVP_get_digestbyname(md), NULL);
	for (int i = 0; i < n; i++)
		EVP_DigestUpdate(c, parts[i], lens[i]);
	EVP_DigestFinal_ex(c, out, NULL);
	EVP_MD_CTX_free(c);
}

/* SaltedHash(Salt, Input, Salt1, Salt2) = MD5(S + SHA1(Input + Salt + Salt1 + Salt2)) */
static void salted_hash(const BYTE* s, const BYTE* in, size_t in_len, const BYTE* s1, const BYTE* s2,
                        BYTE* out)
{
	BYTE sha[20];
	const BYTE* p[] = { in, s, s1, s2 };
	const size_t l[] = { in_len, 48, 32, 32 };
	digest("SHA1", p, l, 4, sha);
	const BYTE* q[] = { s, sha };
	const size_t m[] = { 48, 20 };
	digest("MD5", q, m, 2, out);
}

/* security_A / security_X: three premaster hashes with (ClientRandom, ServerRandom). */
static void hash48(const BYTE* s, const char* a, const char* b, const char* c, const BYTE* cr,
                   const BYTE* sr, BYTE* out)
{
	salted_hash(s, (const BYTE*)a, strlen(a), cr, sr, out);
	salted_hash(s, (const BYTE*)b, strlen(b), cr, sr, out + 16);
	salted_hash(s, (const BYTE*)c, strlen(c), cr, sr, out + 32);
}

static void md5_16_32_32(const BYTE* in0, const BYTE* in1, const BYTE* in2, BYTE* out)
{
	const BYTE* p[] = { in0, in1, in2 };
	const size_t l[] = { 16, 32, 32 };
	digest("MD5", p, l, 3, out);
}

/* security_establish_keys (client side). */
static void establish(const BYTE* cr, const BYTE* sr, int bits, BYTE* sign, BYTE* enc, BYTE* dec,
                      size_t* key_len)
{
	BYTE pre[48], master[48], blob[48];
	memcpy(pre, cr, 24);
	memcpy(pre + 24, sr, 24);
	hash48(pre, "A", "BB", "CCC", cr, sr, master);
	hash48(master, "X", "YY", "ZZZ", cr, sr, blob);
	memcpy(sign, blob, 16);
	md5_16_32_32(blob + 16, cr, sr, dec);
	md5_16_32_32(blob + 32, cr, sr, enc);
	*key_len = 16;
	if (bits == 40)
	{
		memcpy(sign, salt, 3), memcpy(dec, salt, 3), memcpy(enc, salt, 3);
		*key_len = 8;
	}
	else if (bits == 56)
	{
		memcpy(sign, salt, 1), memcpy(dec, salt, 1), memcpy(enc, salt, 1);
		*key_len = 8;
	}
}

/* security_mac_signature / security_salted_mac_signature. */
static void mac(const BYTE* key, size_t key_len, const BYTE* data, unsigned len, const unsigned* count,
                BYTE* out)
{
	BYTE len_le[4] = { len & 0xff, (len >> 8) & 0xff, (len >> 16) & 0xff, len >> 24 };
	BYTE sha[20], md[16];
	BYTE c[4] = { 0 };
	if (count)
		c[0] = *count & 0xff, c[1] = (*count >> 8) & 0xff, c[2] = (*count >> 16) & 0xff,
		c[3] = *count >> 24;
	const BYTE* p[] = { key, pad1, len_le, data, c };
	const size_t l[] = { key_len, 40, 4, len, 4 };
	digest("SHA1", p, l, count ? 5 : 4, sha);
	const BYTE* q[] = { key, pad2, sha };
	const size_t m[] = { key_len, 48, 20 };
	digest("MD5", q, m, 3, md);
	memcpy(out, md, 8);
}

typedef struct
{
	BYTE s[256];
	BYTE i, j;
} rc4_t;

static void rc4_init(rc4_t* r, const BYTE* key, size_t len)
{
	for (int i = 0; i < 256; i++)
		r->s[i] = (BYTE)i;
	BYTE j = 0;
	for (int i = 0; i < 256; i++)
	{
		j = (BYTE)(j + r->s[i] + key[i % len]);
		BYTE t = r->s[i];
		r->s[i] = r->s[j];
		r->s[j] = t;
	}
	r->i = r->j = 0;
}

static void rc4(rc4_t* r, BYTE* d, size_t n)
{
	for (size_t k = 0; k < n; k++)
	{
		r->i++;
		r->j = (BYTE)(r->j + r->s[r->i]);
		BYTE t = r->s[r->i];
		r->s[r->i] = r->s[r->j];
		r->s[r->j] = t;
		d[k] ^= r->s[(BYTE)(r->s[r->i] + r->s[r->j])];
	}
}

/* security_key_update. */
static void key_update(BYTE* key, const BYTE* update_key, size_t key_len, int bits)
{
	BYTE sha[20];
	const BYTE* p[] = { update_key, pad1, key };
	const size_t l[] = { key_len, 40, key_len };
	digest("SHA1", p, l, 3, sha);
	const BYTE* q[] = { update_key, pad2, sha };
	const size_t m[] = { key_len, 48, 20 };
	digest("MD5", q, m, 3, key);
	rc4_t r;
	rc4_init(&r, key, key_len);
	rc4(&r, key, key_len);
	if (bits == 40)
		memcpy(key, salt, 3);
	else if (bits == 56)
		memcpy(key, salt, 1);
}

static void hex(const char* name, const BYTE* b, size_t n)
{
	printf("%s ", name);
	for (size_t i = 0; i < n; i++)
		printf("%02x", b[i]);
	printf("\n");
}

int main(void)
{
	BYTE cr[32], sr[32], data[16];
	for (int i = 0; i < 32; i++)
		cr[i] = (BYTE)i, sr[i] = (BYTE)(0x80 + i);
	for (int i = 0; i < 16; i++)
		data[i] = (BYTE)(0xa0 + i);

	const int all[] = { 40, 56, 128 };
	for (int b = 0; b < 3; b++)
	{
		BYTE sign[16], enc[16], dec[16], init[16], buf[16], sig[8];
		size_t kl;
		char name[32];
		establish(cr, sr, all[b], sign, enc, dec, &kl);
		snprintf(name, sizeof name, "mac%d", all[b]);
		hex(name, sign, kl);
		snprintf(name, sizeof name, "enc%d", all[b]);
		hex(name, enc, kl);
		snprintf(name, sizeof name, "dec%d", all[b]);
		hex(name, dec, kl);
		mac(sign, kl, data, sizeof data, NULL, sig);
		snprintf(name, sizeof name, "sig%d", all[b]);
		hex(name, sig, 8);
		unsigned seven = 7;
		mac(sign, kl, data, sizeof data, &seven, sig);
		snprintf(name, sizeof name, "salted%d", all[b]);
		hex(name, sig, 8);

		/* security_encrypt over 4098 packets: update before the 4097th. */
		rc4_t r;
		memcpy(init, enc, 16);
		rc4_init(&r, enc, kl);
		unsigned use = 0;
		for (int pkt = 0; pkt < 4098; pkt++)
		{
			if (use >= 4096)
			{
				key_update(enc, init, kl, all[b]);
				rc4_init(&r, enc, kl);
				use = 0;
			}
			memcpy(buf, data, sizeof buf);
			rc4(&r, buf, sizeof buf);
			use++;
			if (pkt == 0 || pkt == 4095 || pkt == 4096 || pkt == 4097)
			{
				snprintf(name, sizeof name, "ct%d_%d", all[b], pkt);
				hex(name, buf, sizeof buf);
			}
		}
	}

	/* crypto_rsa_public_encrypt: little-endian input and output, modulus width. */
	BIGNUM *n = NULL, *e = NULL, *m = BN_new(), *c = BN_new();
	BN_CTX* ctx = BN_CTX_new();
	BN_hex2bn(&n, "C3C4E2C2B1B9C5F2D3E1C0B5A4F7E6D9C8B7A6F5E4D3C2B1A0F9E8D7C6B5A49"
	              "384736251403F2E1D0C1B2A3948576A5B4C3D2E1F0A9B8C7D6E5F4031425364758697A8B9");
	BN_hex2bn(&e, "10001");
	BYTE be[32], out[512] = { 0 };
	for (int i = 0; i < 32; i++)
		be[i] = cr[31 - i];
	BN_bin2bn(be, 32, m);
	BN_mod_exp(c, m, e, n, ctx);
	int len = BN_num_bytes(n);
	BYTE tmp[512] = { 0 };
	BN_bn2binpad(c, tmp, len);
	for (int i = 0; i < len; i++)
		out[i] = tmp[len - 1 - i];
	hex("rsa", out, (size_t)len);
	return 0;
}
