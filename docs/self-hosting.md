# Self-Hosting DarshanDB

DarshanDB runs anywhere you can run a single binary and connect to PostgreSQL.

## Docker (Recommended)

```bash
curl -fsSL https://darshandb.dev/docker -o docker-compose.yml
docker compose up -d
```

The default `docker-compose.yml` includes DarshanDB and PostgreSQL 16 with pgvector.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | — | PostgreSQL connection string |
| `DARSHAN_PORT` | `7700` | Server listen port |
| `DARSHAN_ADMIN_DIR` | `/usr/share/darshan/admin` | Admin dashboard static files |
| `DARSHAN_JWT_SECRET` | auto-generated | JWT signing key (RS256) |
| `DARSHAN_STORAGE_BACKEND` | `local` | `local`, `s3`, `r2`, `minio` |
| `DARSHAN_S3_BUCKET` | — | S3 bucket name |
| `DARSHAN_S3_REGION` | — | S3 region |
| `RUST_LOG` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |

## Bare Metal

### Requirements
- PostgreSQL 16+ with pgvector extension
- ~30MB disk for the binary
- 256MB RAM minimum (1GB recommended)

### Install

```bash
curl -fsSL https://darshandb.dev/install | sh
```

### Configure

```bash
export DATABASE_URL="postgres://user:pass@localhost:5432/darshandb"
darshan start --prod
```

## Kubernetes

```bash
helm repo add darshan https://charts.darshandb.dev
helm install darshan darshan/darshandb \
  --set postgres.enabled=true \
  --set postgres.storageClass=ssd \
  --set replicas=3 \
  --set ingress.enabled=true \
  --set ingress.host=api.example.com
```

### Helm Values

```yaml
replicas: 3
image:
  repository: ghcr.io/darshjme/darshandb
  tag: latest

postgres:
  enabled: true
  storageClass: ssd
  size: 50Gi

ingress:
  enabled: true
  host: api.example.com
  tls: true

resources:
  requests:
    cpu: 250m
    memory: 512Mi
  limits:
    cpu: "2"
    memory: 2Gi
```

## Backups

```bash
# Manual backup
darshan backup --output /backups/darshan-$(date +%Y%m%d).sql.gz

# Restore from backup
darshan restore --input /backups/darshan-20260405.sql.gz
```

## Monitoring

DarshanDB exposes Prometheus metrics at `/metrics`:

```bash
# Check server health
curl http://localhost:7700/api/admin/health

# Prometheus metrics
curl http://localhost:7700/metrics
```

Grafana dashboard templates are included in the `deploy/` directory.
