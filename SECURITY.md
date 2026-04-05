# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | Yes                |

We release patches for security vulnerabilities in the latest minor version. Older versions do not receive backported fixes.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Instead, report vulnerabilities privately using one of these methods:

1. **GitHub Security Advisories** (preferred): [Report a vulnerability](https://github.com/darshjme/darshjdb/security/advisories/new)
2. **Email**: security@db.darshj.me

### What to Include

- Description of the vulnerability
- Steps to reproduce
- Affected version(s)
- Potential impact
- Suggested fix (if any)

### Response Timeline

| Stage | Timeframe |
|-------|-----------|
| Acknowledgment | Within 48 hours |
| Initial assessment | Within 5 business days |
| Fix development | Within 30 days for critical, 90 days for others |
| Public disclosure | After fix is released |

### What to Expect

1. You will receive an acknowledgment within 48 hours.
2. We will work with you to understand the issue and assess its severity.
3. We will develop and test a fix privately.
4. We will release the fix and publish a security advisory.
5. You will be credited in the advisory (unless you prefer to remain anonymous).

## Security Architecture

DarshJDB implements 11 layers of defense-in-depth security. See [docs/security.md](docs/security.md) for the full architecture.

Key highlights:

- **TLS 1.3 mandatory** -- no plaintext, no TLS 1.2 fallback
- **Argon2id** password hashing (PHC winner)
- **RS256 + Ed25519** JWT signing
- **AES-256-GCM** encryption at rest
- **Row-level security** -- unauthorized data never leaves the database
- **V8 sandboxing** -- server functions isolated from the system
- **Zero-trust default** -- everything denied unless explicitly allowed

## Dependency Policy

- All dependencies are pinned to exact versions in lock files.
- We run `cargo audit` and `npm audit` in CI on every pull request.
- Critical dependency vulnerabilities are treated as project security issues.

## Scope

The following are in scope for security reports:

- DarshJDB server (`packages/server/`)
- DarshJDB CLI (`packages/cli/`)
- Client SDKs (`packages/client-core/`, `packages/react/`, `packages/nextjs/`, `packages/angular/`)
- Admin dashboard (`packages/admin/`)
- PHP SDK (`sdks/php/`)
- Python SDK (`sdks/python/`)
- Docker images and deployment configurations

The following are out of scope:

- Example applications (`examples/`)
- Documentation website
- Third-party integrations not maintained by the DarshJDB team

## Production Deployment Security Checklist

Before deploying DarshJDB to production, verify:

- [ ] `DDB_JWT_SECRET` is set to a cryptographically random value (minimum 32 characters, **not** `change-me-in-production`)
- [ ] `POSTGRES_PASSWORD` is set to a strong unique password (not the default `ddb`)
- [ ] Postgres port (`5432`) is **not** exposed to the host network (internal Docker network only)
- [ ] CORS allowed origins are configured for your frontend domains
- [ ] TLS termination is enabled (via reverse proxy or load balancer)
- [ ] Function runtime permissions are reviewed and restricted (Deno `--allow-env`, `--allow-read`, `--allow-net`)
- [ ] `.env` files are excluded from version control (verify `.gitignore`)
- [ ] `RUST_LOG` is set to `info` (not `debug`) in production
- [ ] Backup outputs are encrypted at rest
- [ ] Monitoring and alerting configured for `429`, `401`, and `403` response codes
- [ ] `docker-compose.dev.yml` is **never** used in production or staging environments
