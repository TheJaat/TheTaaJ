# Era 29 · Cryptography and encryption

*Part of the work list. The rules, who is doing what now, and what waits for the user are in [`docs/who-does-what.md`](../who-does-what.md); the lanes in [`docs/lanes.md`](../lanes.md).*


**Done when**: the disk is encrypted, its key unlocked at login and sealed to the TPM where there is one; an HTTPS page is fetched with TLS 1.3; packages and programs are verified by their signatures; swap is encrypted; and every algorithm in our library passes its RFC test vectors and Google's Wycheproof cases and gives the same answers as an audited library on the host, in every test run. Whose it is, and when, is for the user to say. Pointed to, not moved: K37 (TLS 1.3), K109 (disk encryption), K106 and K107 (Era 18), K108 (passwords), J101 (Era 26), E5 (swap), Era 8's XSAVE.

**Our own and an audited one, both (the user's decision, 2026-11-26).** We write every algorithm ourselves, to learn it, and check it three ways: the RFCs' test vectors, Wycheproof's cases (the edge cases that break real implementations), and differential tests on the host against an audited library (RustCrypto's crates, `rustls`, or Monocypher/BearSSL in C), the same inputs to both on every run. Real secrets are protected by the audited library until ours has passed all of that and a review of its timing; then ours may take over, one algorithm at a time.

What is right: the kernel's random generator is ChaCha20 (RFC 8439, `chacha.rs`, `random.rs`), seeded from RDSEED or RDRAND (RNDR on AArch64), timing jitter and the clock, stirred every tick, with fast key erasure - each request replaces the key, so the kernel's memory read later can't tell what was handed out before. Programs get it through `RANDOM` (`getrandom`), which never blocks. The design Linux and OpenBSD use.

What is missing: everything else. No hash, no authenticated encryption, no key exchange, no signatures, no key derivation, no password hashing; so no disk encryption, no TLS, no signed programs or packages, no SSH; no place to keep keys; no use of the CPU's crypto instructions (AES-NI, SHA, ARMv8 Crypto), which need the vector state the kernel doesn't save yet (Era 8's XSAVE); no constant-time checks; secrets not wiped after use.

All of it stays out of the kernel but the random generator (there already) and saving the vector registers crypto instructions use: the algorithms are a user-space library (`no_std` Rust, with C names for libc); keys live in a key service; disk encryption is a block service; TLS is a library programs use.

| # | Item | Size | Notes |
|---|---|---|---|
| K176 | **Our crypto library**: SHA-256/512, BLAKE2s/b, HMAC, HKDF, ChaCha20-Poly1305, AES-128/256-GCM, X25519, Ed25519, Argon2id; `no_std` Rust with a C interface; RFC test vectors and Wycheproof as host tests | L | First |
| K177 | **The audited library beside it**: one ported (RustCrypto, `rustls`, or Monocypher/BearSSL), behind the same interface, so a program or service can use either | M | With K176; real secrets use this one until K180 |
| K178 | **Differential tests**: random inputs to ours and the audited one on every host test run; any difference fails the build | S | After both |
| K179 | **Constant time and wiping**: a tool on the host that checks the secret-dependent code takes the same path and time (dudect-style, or checking the machine code for branches on secrets); secrets zeroed after use, in the library and the services | M | |
| K180 | **Ours takes over**, an algorithm at a time, once it passes the vectors, Wycheproof, the differential tests and K179, with its journal entry saying so | S | Last |
| K181 | **The CPU's crypto instructions**: AES-NI, the SHA extensions, ARMv8 Crypto, used when present, the same answers as without them | M | After Era 8's XSAVE (vector registers saved per thread) |
| K182 | **A key service**: a program gets a capability to *use* a key (encrypt, sign, derive), never the key itself; keys unlocked at login, sealed to the TPM where there is one | L | After K108 |
| K183 | **Encrypted swap**, with a key made at each boot and never stored | S | With E5 |
| K184 | **Encryption per file or per user in TaajFS version 7**: its checksums (J74) become authenticated encryption, so the tree is encrypted and tamper-evident from the superblock down (as ZFS's native encryption) | L | With Era 14's version 7; beside K109's whole-disk encryption |
| K185 | **SSH**: a server and a client, keys from the key service | L | After K37 and K182 |
