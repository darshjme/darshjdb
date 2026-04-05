# Self-Hosting DarshanDB

DarshanDB runs anywhere you can run a single binary and connect to PostgreSQL.

## Deployment Topology

```mermaid
graph TB
    subgraph Internet
        Users[Users / Clients]
    end

    subgraph Edge["Edge / Load Balancer"]
        LB[nginx / Caddy / Cloudflare]
    end

    subgraph App["Application Tier"]
        D1[DarshanDB Instance 1]
        D2[DarshanDB Instance 2]
        D3[DarshanDB Instance N]
    end

    subgraph Data["Data Tier"]
        PG[(PostgreSQL 16+<br/>Primary)]
        PGR[(PostgreSQL<br/>Read Replica)]
        S3[S3 / R2 / MinIO<br/>File Storage]
    end

    subgraph Monitoring["Monitoring"]
        PROM[Prometheus]
        GRAF[Grafana]
    end

    Users --> LB
    LB -->|TLS termination| D1
    LB --> D2
    LB --> D3
    D1 --> PG
    D2 --> PG
    D3 --> PG
    PG -->|Streaming replication| PGR
    D1 --> S3
    D2 --> S3
    D3 --> S3
    D1 -->|/metrics| PROM
    D2 -->|/metrics| PROM
    D3 -->|/metrics| PROM
    PROM --> GRAF

    style Edge fill:#F59E0B,color:#000
    style App fill:#1a1a2e,stroke:#F59E0B,color:#fff
    style Data fill:#336791,stroke:#fff,color:#fff
    style Monitoring fill:#16213e,stroke:#F59E0B,color:#fff
```

## Docker (Recommended)

```bash
curl -fsSL https://darshandb.dev/docker -o docker-compose.yml
docker compose up -d
```

The default `docker-compose.yml` includes DarshanDB and PostgreSQL 16 with pgvector.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | -- | PostgreSQL connection string |
| `DARSHAN_PORT` | `7700` | Server listen port |
| `DARSHAN_ADMIN_DIR` | `/usr/share/darshan/admin` | Admin dashboard static files |
| `DARSHAN_JWT_SECRET` | auto-generated | JWT signing key (RS256) |
| `DARSHAN_STORAGE_BACKEND` | `local` | `local`, `s3`, `r2`, `minio` |
| `DARSHAN_S3_BUCKET` | -- | S3 bucket name |
| `DARSHAN_S3_REGION` | -- | S3 region |
| `DARSHAN_CORS_ORIGINS` | `*` (dev) / none (prod) | Allowed CORS origins |
| `DARSHAN_ENCRYPTION_KEY` | -- | AES-256-GCM key for at-rest encryption |
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

### systemd Service

```ini
# /etc/systemd/system/darshandb.service
[Unit]
Description=DarshanDB Server
After=postgresql.service

[Service]
Type=simple
User=darshandb
Environment=DATABASE_URL=postgres://user:pass@localhost:5432/darshandb
Environment=DARSHAN_PORT=7700
Environment=RUST_LOG=warn
ExecStart=/usr/local/bin/darshan start --prod
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable darshandb
sudo systemctl start darshandb
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

env:
  RUST_LOG: warn
  DARSHAN_PG_POOL_SIZE: "20"
```

## Reverse Proxy Configuration

### nginx

```nginx
upstream darshandb {
    server 127.0.0.1:7700;
    keepalive 64;
}

server {
    listen 443 ssl http2;
    server_name api.example.com;

    ssl_certificate /etc/letsencrypt/live/api.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.example.com/privkey.pem;

    # WebSocket support
    location / {
        proxy_pass http://darshandb;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 86400s;  # 24h for WebSocket
        proxy_send_timeout 86400s;
    }
}
```

### Caddy

```
api.example.com {
    reverse_proxy localhost:7700
}
```

Caddy handles TLS, WebSocket upgrades, and HTTP/2 automatically.

## Backups

### Manual Backup

```bash
# Compressed SQL dump
darshan backup --output /backups/darshan-$(date +%Y%m%d).sql.gz

# Restore from backup
darshan restore --input /backups/darshan-20260405.sql.gz

# Verify backup integrity
darshan backup verify --input /backups/darshan-20260405.sql.gz
```

### Automated Backup with Cron

```bash
# /etc/cron.d/darshandb-backup
# Daily backup at 2 AM, keep 30 days
0 2 * * * darshandb /usr/local/bin/darshan backup \
  --output /backups/darshan-$(date +\%Y\%m\%d).sql.gz && \
  find /backups -name "darshan-*.sql.gz" -mtime +30 -delete
```

### S3 Backup

```bash
# Backup directly to S3
darshan backup --output s3://my-backups/darshan-$(date +%Y%m%d).sql.gz \
  --s3-region us-east-1

# Restore from S3
darshan restore --input s3://my-backups/darshan-20260405.sql.gz \
  --s3-region us-east-1
```

### Point-in-Time Recovery

For mission-critical deployments, enable PostgreSQL WAL archiving:

```bash
# postgresql.conf
archive_mode = on
archive_command = 'aws s3 cp %p s3://my-wal-archive/%f'
```

This allows restoring to any point in time, not just the last backup.

## Monitoring

### Prometheus Metrics

DarshanDB exposes Prometheus metrics at `/metrics`:

```bash
curl http://localhost:7700/metrics
```

Key metrics:

| Metric | Type | Description |
|--------|------|-------------|
| `darshandb_queries_total` | Counter | Total queries processed |
| `darshandb_mutations_total` | Counter | Total mutations processed |
| `darshandb_query_duration_seconds` | Histogram | Query latency distribution |
| `darshandb_mutation_duration_seconds` | Histogram | Mutation latency distribution |
| `darshandb_websocket_connections` | Gauge | Active WebSocket connections |
| `darshandb_subscriptions_active` | Gauge | Active live query subscriptions |
| `darshandb_pg_pool_connections` | Gauge | PostgreSQL connection pool usage |
| `darshandb_storage_bytes_total` | Counter | Total bytes stored |
| `darshandb_auth_failures_total` | Counter | Failed authentication attempts |

### Grafana Dashboard

Import the included Grafana dashboard from `deploy/grafana-dashboard.json`:

```bash
# Or download from the release
curl -fsSL https://darshandb.dev/grafana-dashboard.json \
  -o /var/lib/grafana/dashboards/darshandb.json
```

### Health Check

```bash
# Quick health check
curl http://localhost:7700/api/admin/health

# Use in Docker healthcheck
healthcheck:
  test: ["CMD", "curl", "-f", "http://localhost:7700/api/admin/health"]
  interval: 30s
  timeout: 5s
  retries: 3
```

### Alerting Rules (Prometheus)

```yaml
# prometheus-rules.yml
groups:
  - name: darshandb
    rules:
      - alert: HighQueryLatency
        expr: histogram_quantile(0.99, darshandb_query_duration_seconds) > 1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "DarshanDB P99 query latency above 1s"

      - alert: HighAuthFailures
        expr: rate(darshandb_auth_failures_total[5m]) > 10
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "High rate of authentication failures (possible brute force)"

      - alert: ConnectionPoolExhausted
        expr: darshandb_pg_pool_connections / darshandb_pg_pool_max > 0.9
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "PostgreSQL connection pool over 90% utilization"
```

## Upgrading

See the [Migration Guide](migration.md) for instructions on upgrading between DarshanDB versions, managing database migrations, and handling breaking changes.

---

[Previous: Getting Started](getting-started.md) | [Next: Query Language](query-language.md) | [All Docs](README.md)
