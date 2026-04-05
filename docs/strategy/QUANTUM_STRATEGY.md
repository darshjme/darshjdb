# DarshanDB Quantum Resistance Strategy

> Based on: "Quantum Blockchain: Trends, Technologies, and Future Directions"
> (IET Quantum Communication, 2024, DOI: 10.1049/qtc2.12119)

---

## 1. Current Cryptographic Inventory

Every cryptographic primitive in DarshanDB, assessed against known quantum attacks.

| Component | Algorithm | Location | Quantum Status | Attack Vector |
|-----------|-----------|----------|---------------|---------------|
| JWT signing (production) | RS256 (RSA-2048 + SHA-256) | `auth/session.rs` KeyManager | **BROKEN** | Shor's algorithm factors RSA in polynomial time |
| JWT signing (dev) | HS256 (HMAC-SHA256) | `auth/session.rs` KeyManager::from_secret | **WEAKENED** | Grover's gives O(2^128) from O(2^256) -- still safe at 128-bit |
| Password hashing | Argon2id (64MB, t=3, p=4) | `auth/password.rs` | **SAFE** | Memory-hard; Grover's speedup irrelevant against memory bottleneck |
| OAuth state signing | HMAC-SHA256 | `auth/oauth.rs` | **WEAKENED** | Grover's quadratic speedup: 256-bit -> 128-bit effective security |
| Signed URLs | HMAC-SHA256 | `storage/signed_urls.rs` | **WEAKENED** | Same as OAuth state; 128-bit effective still practically safe |
| Refresh token storage | SHA-256 hash of random bytes | `auth/session.rs` hex_sha256 | **WEAKENED** | Pre-image resistance reduced from 256 to 128 bits; still safe |
| Refresh token generation | 32 random bytes (OsRng) | `auth/session.rs` create_session | **SAFE** | No algebraic structure to exploit |
| Rate limit keys | SHA-256 | `middleware/rate_limit.rs` | **WEAKENED** | Grover's; but rate limit keys are ephemeral, risk is negligible |
| Device fingerprint hash | SHA-256 | `auth/session.rs` hex_sha256 | **WEAKENED** | Same Grover reduction; pre-image attack impractical at 128-bit |

### Risk Summary

- **Critical (Shor's):** RS256 JWT signing. RSA-2048 is completely broken by a sufficiently large quantum computer. This is the primary migration target.
- **Low (Grover's):** All SHA-256/HMAC-SHA256 uses drop from 256-bit to ~128-bit effective security. This remains computationally infeasible but violates the principle of maintaining a >=128-bit post-quantum security margin.
- **None:** Argon2id, random token generation.

---

## 2. Migration Path to Post-Quantum Cryptography

Three-phase migration aligned with NIST PQC standardization timeline.

### Phase 1: Immediate Hardening (Now)

**Goal:** Maximize classical security margins so Grover's reduction stays above 128-bit.

| Action | Detail | Effort |
|--------|--------|--------|
| Upgrade internal hashes to SHA-512 | `hex_sha256` -> `hex_sha512` for refresh token storage, device fingerprints, rate limit keys | Low -- single function change |
| Increase HMAC to SHA-512 | OAuth state and signed URL HMAC upgraded to HMAC-SHA512 | Low -- config change in hmac crate |
| Document algorithm in JWT `alg` header | Already done (RS256/HS256 in header) | None |
| Add `algorithm` field to config | Allow runtime selection of signing algorithm without code changes | Medium |

**Security gain:** All symmetric primitives move from 128-bit post-quantum to 256-bit post-quantum effective security.

### Phase 2: Hybrid Post-Quantum Mode (When NIST PQC Libraries Mature in Rust)

**Goal:** Add CRYSTALS-Dilithium as a parallel signing algorithm alongside RS256.

| Action | Detail | Effort |
|--------|--------|--------|
| Add `dilithium3` variant to KeyManager | New enum variant in `algorithm` field; KeyManager already supports algorithm selection | Medium |
| Hybrid JWT signing | Sign with BOTH RS256 and Dilithium3; include both signatures in JWT | Medium |
| Hybrid JWT verification | Accept token if EITHER signature verifies (graceful migration) | Medium |
| CRYSTALS-Dilithium key generation | Add key generation for Dilithium3 key pairs | Low (library call) |
| Audit log signing | Sign audit entries with Dilithium3 (see Section 5) | Medium |

**Candidate Rust crates (do NOT add yet):**
- `pqcrypto-dilithium` -- Pure Rust CRYSTALS-Dilithium
- `pqcrypto-falcon` -- Pure Rust Falcon (alternative lattice scheme)
- `oqs` -- Open Quantum Safe bindings (liboqs)

**Why hybrid?** The paper (Section 4.2) recommends hybrid schemes because:
1. PQC algorithms are newer and less battle-tested than RSA/ECDSA
2. Hybrid ensures security even if one scheme is broken
3. Backward compatibility with clients that don't support PQC

### Phase 3: Full Post-Quantum (When Quantum Computers Threaten RSA-2048)

**Goal:** Remove classical asymmetric crypto entirely.

| Action | Detail | Effort |
|--------|--------|--------|
| Remove RS256 signing path | KeyManager drops RSA support | Low |
| Dilithium3-only JWT | `alg: "dilithium3"` becomes the sole signing algorithm | Low |
| Remove RSA key rotation | Simplify KeyManager (Dilithium keys are smaller) | Low |
| Falcon as backup | If Dilithium is compromised, Falcon is the fallback lattice scheme | Medium |
| Hash-based signatures (SPHINCS+) | Stateless hash-based signatures as ultimate fallback (larger but conservative) | Medium |

**Timeline estimate:** Phase 3 is triggered when NIST or NSA issues guidance that RSA-2048 has < 10 years of security margin. Current estimates: 2035-2045.

---

## 3. Quantum-Resistant JWT Design

### Current JWT Structure (RS256)

```
Header:  { "alg": "RS256", "kid": "key-1", "typ": "JWT" }
Payload: { "sub": "user-id", "sid": "session-id", "roles": [...], ... }
Signature: RSA-SHA256(header.payload, private_key)
```

### Phase 2 Hybrid JWT Structure

```
Header: {
  "alg": "RS256+DILITHIUM3",
  "kid": "key-1",
  "typ": "JWT",
  "pqalg": "dilithium3",
  "pqkid": "pq-key-1"
}
Payload: { "sub": "user-id", "sid": "session-id", "roles": [...], ... }
Signature: {
  "classical": RSA-SHA256(header.payload, rsa_private_key),
  "pq": DILITHIUM3(header.payload, dilithium_private_key)
}
```

### Verification Logic (Phase 2)

```
fn validate_hybrid_token(token) -> Result<Claims>:
    if has_pq_signature(token):
        // Hybrid mode: accept if EITHER verifies
        classical_ok = verify_rsa(token)
        pq_ok = verify_dilithium(token)
        if classical_ok || pq_ok:
            return Ok(claims)
        return Err(InvalidSignature)
    else:
        // Legacy mode: RS256 only (backward compat)
        return verify_rsa(token)
```

### Phase 3 JWT Structure

```
Header:  { "alg": "DILITHIUM3", "kid": "pq-key-2", "typ": "JWT" }
Payload: { "sub": "user-id", "sid": "session-id", "roles": [...], ... }
Signature: DILITHIUM3(header.payload, dilithium_private_key)
```

### Key Size Comparison

| Algorithm | Public Key | Signature | Security Level |
|-----------|-----------|-----------|----------------|
| RSA-2048 | 256 bytes | 256 bytes | 112-bit classical, **0-bit quantum** |
| CRYSTALS-Dilithium3 | 1,952 bytes | 3,293 bytes | 128-bit classical, 128-bit quantum |
| Falcon-512 | 897 bytes | 666 bytes | 128-bit classical, 128-bit quantum |
| SPHINCS+-SHA256-128f | 32 bytes | 17,088 bytes | 128-bit classical, 128-bit quantum |

**Trade-off:** Dilithium3 signatures are ~13x larger than RSA-2048. For JWTs transmitted on every request, this increases token size from ~800 bytes to ~4,500 bytes. Acceptable for API traffic; may need compression for high-frequency WebSocket messages.

---

## 4. Data Integrity Anchoring (Merkle Trees)

### Design

Every write transaction in DarshanDB produces a Merkle root hash anchoring the transaction's changes.

```
Transaction: INSERT user { name: "Alice", email: "alice@example.com" }

Leaf nodes:
  H1 = SHA-512("entity_id:uuid-1")
  H2 = SHA-512("attribute:name|value:Alice")
  H3 = SHA-512("attribute:email|value:alice@example.com")

Internal nodes:
  H4 = SHA-512(H1 || H2)
  H5 = SHA-512(H3 || H3)  // duplicate for odd count

Merkle root:
  ROOT = SHA-512(H4 || H5)
```

### Why SHA-512?

- SHA-256 provides 128-bit post-quantum security (Grover's)
- SHA-512 provides 256-bit post-quantum security (Grover's)
- The paper (Section 3.1) recommends doubling hash output sizes for quantum resistance
- SHA-512 is actually faster than SHA-256 on 64-bit CPUs (wider internal state matches register width)

### Optional External Anchoring

Merkle roots can optionally be anchored to an external blockchain or timestamping service:

```
Anchor record:
  merkle_root: SHA-512 hash
  timestamp: RFC 3339
  block_height: (if blockchain anchored)
  tx_hash: (if blockchain anchored)
```

This provides tamper-evidence even if DarshanDB's database is compromised.

### Implementation Location

- `packages/server/src/triple_store/transaction.rs` -- compute Merkle root per transaction
- `packages/server/src/triple_store/merkle.rs` -- new module for Merkle tree construction
- Configuration: `merkle_anchoring: { enabled: bool, backend: "log" | "ethereum" | "bitcoin" }`

---

## 5. Quantum-Safe Audit Trail

### Hash-Chained Audit Log

Every audit log entry is linked to the previous entry via a hash chain, creating a tamper-evident sequence.

```
Entry N:
  event: "user.login"
  actor: "user-uuid"
  timestamp: "2024-01-15T10:30:00Z"
  data: { ip: "1.2.3.4", ... }
  prev_hash: SHA-512(Entry N-1)
  signature: DILITHIUM3(SHA-512(event || actor || timestamp || data || prev_hash))
  hash: SHA-512(event || actor || timestamp || data || prev_hash || signature)
```

### Properties

1. **Tamper-evident:** Modifying any entry breaks the hash chain from that point forward
2. **Quantum-resistant:** SHA-512 hashes resist Grover's; Dilithium signatures resist Shor's
3. **Verifiable:** Any party with the public key can verify the entire chain
4. **Append-only:** New entries reference the previous hash; cannot insert or reorder

### Implementation Phases

- **Phase 1 (now):** Hash chain with SHA-512, no signatures (still tamper-evident)
- **Phase 2:** Add Dilithium3 signatures to each entry
- **Phase 3:** Periodic Merkle root anchoring of audit log segments

---

## 6. Implementation Roadmap

### Timeline

```
2024-2025 (Phase 1 - Immediate Hardening)
  [x] Document quantum strategy (this file)
  [x] Add quantum readiness annotations to codebase
  [ ] Upgrade hex_sha256 -> hex_sha512 for internal hashes
  [ ] Upgrade HMAC-SHA256 -> HMAC-SHA512 for OAuth state and signed URLs
  [ ] Add algorithm configuration field to DarshanDB config
  [ ] Implement SHA-512 Merkle tree for transaction integrity
  [ ] Implement hash-chained audit log (SHA-512, no PQ signatures yet)

2025-2026 (Phase 2 - Hybrid PQC)
  [ ] Evaluate Rust PQC crates (pqcrypto-dilithium, oqs)
  [ ] Add Dilithium3 key generation and storage
  [ ] Implement hybrid JWT signing (RS256 + Dilithium3)
  [ ] Implement hybrid JWT verification
  [ ] Add Dilithium3 signatures to audit log entries
  [ ] Update JWKS endpoint to publish Dilithium3 public keys
  [ ] Performance benchmarking: Dilithium3 sign/verify latency
  [ ] Client SDK updates for larger token handling

2030+ (Phase 3 - Full PQC, triggered by threat assessment)
  [ ] Remove RS256 signing path
  [ ] Dilithium3-only JWT issuance
  [ ] Falcon as backup algorithm
  [ ] SPHINCS+ as conservative fallback
  [ ] Remove all RSA key management code
```

### Decision Criteria for Phase Transitions

| Trigger | Action |
|---------|--------|
| NIST finalizes FIPS 204 (ML-DSA/Dilithium) | Begin Phase 2 implementation |
| Rust `pqcrypto` crates reach 1.0 with audits | Integrate into DarshanDB |
| NSA/NIST issues "harvest now, decrypt later" advisory | Accelerate Phase 2 |
| Quantum computer breaks RSA-2048 in practice | Emergency Phase 3 |
| RSA-2048 estimated < 10 years secure | Planned Phase 3 migration |

### Risk: "Harvest Now, Decrypt Later"

The paper (Section 2.3) highlights that adversaries may record encrypted traffic today to decrypt it when quantum computers mature. For DarshanDB:

- **JWTs are short-lived (15 min):** Low harvest risk -- tokens are worthless after expiry
- **Refresh tokens are opaque random bytes:** No structure to exploit post-quantum
- **Passwords use Argon2id:** Memory-hard hashing is quantum-safe
- **Stored data uses Postgres encryption:** Migration to PQC TLS is a Postgres/infrastructure concern, not DarshanDB application layer

**Primary risk vector:** Long-lived API keys or service tokens (if added in future). These MUST use PQC from inception.

---

## 7. References

1. M. A. Khan et al., "Quantum blockchain: Trends, technologies, and future directions," *IET Quantum Communication*, 2024. DOI: [10.1049/qtc2.12119](https://doi.org/10.1049/qtc2.12119)
2. NIST Post-Quantum Cryptography Standardization, [https://csrc.nist.gov/projects/post-quantum-cryptography](https://csrc.nist.gov/projects/post-quantum-cryptography)
3. CRYSTALS-Dilithium specification, [https://pq-crystals.org/dilithium/](https://pq-crystals.org/dilithium/)
4. Falcon signature scheme, [https://falcon-sign.info/](https://falcon-sign.info/)
5. SPHINCS+ specification, [https://sphincs.org/](https://sphincs.org/)
6. L. K. Grover, "A fast quantum mechanical algorithm for database search," *STOC 1996*
7. P. W. Shor, "Polynomial-time algorithms for prime factorization and discrete logarithms on a quantum computer," *SIAM J. Comput.*, 1997

---

## 8. Code Integration Points

Files that will require changes during PQC migration:

| File | Phase | Change |
|------|-------|--------|
| `packages/server/src/auth/session.rs` | 1, 2 | SHA-512 hashes, Dilithium KeyManager variant |
| `packages/server/src/auth/oauth.rs` | 1 | HMAC-SHA512 for state signing |
| `packages/server/src/storage/signed_urls.rs` | 1 | HMAC-SHA512 for URL signing |
| `packages/server/src/middleware/rate_limit.rs` | 1 | SHA-512 for rate limit keys (optional, low priority) |
| `packages/server/src/triple_store/transaction.rs` | 1 | Merkle root computation |
| `packages/server/src/triple_store/merkle.rs` | 1 | New module: Merkle tree with SHA-512 |
| `packages/server/src/auth/audit.rs` | 1, 2 | Hash-chained audit log, PQ signatures |
| `Cargo.toml` | 2 | Add `pqcrypto-dilithium` dependency |
| `darshandb.toml` | 1, 2 | Algorithm configuration fields |

---

*This strategy ensures DarshanDB is quantum-ready without premature dependency on immature PQC libraries. The existing KeyManager architecture already supports algorithm selection -- the migration path is incremental, not a rewrite.*
