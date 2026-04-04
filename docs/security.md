# Security Architecture

DarshanDB implements 11 layers of defense-in-depth security.

## Defense-in-Depth Stack

| Layer | What | How |
|-------|------|-----|
| 0 | **TLS 1.3** | Mandatory encryption. No plaintext. No TLS 1.2 fallback. |
| 1 | **Rate Limiting** | Token bucket per IP, per user, per API key. |
| 2 | **Input Validation** | Schema-validated at the API boundary. |
| 3 | **Authentication** | JWT RS256 + refresh tokens + device fingerprint binding. |
| 4 | **Authorization** | Permission engine evaluates every single request. |
| 5 | **Row-Level Security** | SQL WHERE injection — unauthorized data never leaves the DB. |
| 6 | **Field Filtering** | Restricted fields stripped from response server-side. |
| 7 | **Query Complexity** | Rejects queries that would scan too many triples. |
| 8 | **V8 Sandboxing** | Server functions run in isolated V8 contexts. |
| 9 | **Audit Logging** | Every mutation logged with actor, timestamp, and diff. |
| 10 | **Anomaly Detection** | Unusual access patterns trigger alerts. |

## Password Security

- **Argon2id** — winner of the Password Hashing Competition
- Memory: 64MB, iterations: 3, parallelism: 4
- Top 10,000 breached passwords rejected at signup
- Account lockout after 5 failed attempts (30-minute cooldown)

## Token Security

- Access tokens: RS256, 15-minute expiry
- Refresh tokens: opaque 32-byte, 30-day expiry, device-bound
- Key rotation: new keys issued monthly, old keys valid for verification

## Server Function Isolation

- CPU time limit: 30 seconds (configurable)
- Memory limit: 128MB (configurable)
- `fetch()` restricted to domain allowlist
- Private IP ranges blocked (SSRF prevention)
- DNS rebinding protection

## OWASP API Top 10 Coverage

Every risk in the OWASP API Security Top 10 is addressed by design, not by configuration.

See the main README for the full coverage matrix.
