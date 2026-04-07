#!/usr/bin/env bash
# ── Build and tag DarshJDB Docker image ──────────────────────────────
# Usage:
#   ./scripts/docker-build.sh              # tags as :latest
#   ./scripts/docker-build.sh 0.1.0        # tags as :0.1.0 and :latest
#   ./scripts/docker-build.sh 0.1.0 --push # build + push in one step
# ─────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="darshjme/darshjdb"
VERSION="${1:-}"
PUSH="${2:-}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${PROJECT_ROOT}"

# Determine tags
TAGS=("-t" "${REPO}:latest")
if [[ -n "${VERSION}" && "${VERSION}" != "--push" ]]; then
    TAGS+=("-t" "${REPO}:${VERSION}")

    # Also tag major.minor if semver
    if [[ "${VERSION}" =~ ^([0-9]+\.[0-9]+)\.[0-9]+$ ]]; then
        TAGS+=("-t" "${REPO}:${BASH_REMATCH[1]}")
    fi
fi

# Build metadata labels
BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
GIT_SHA="$(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"

echo "==> Building ${REPO}"
echo "    Tags: ${TAGS[*]}"
echo "    Git:  ${GIT_SHA}"
echo ""

docker build \
    "${TAGS[@]}" \
    --label "org.opencontainers.image.created=${BUILD_DATE}" \
    --label "org.opencontainers.image.revision=${GIT_SHA}" \
    --label "org.opencontainers.image.version=${VERSION:-dev}" \
    --build-arg BUILDKIT_INLINE_CACHE=1 \
    --progress=plain \
    .

echo ""
echo "==> Build complete."
docker images "${REPO}" --format "table {{.Repository}}\t{{.Tag}}\t{{.Size}}\t{{.CreatedAt}}"

# Optional push
if [[ "${PUSH}" == "--push" ]] || [[ "${VERSION}" == "--push" ]]; then
    echo ""
    echo "==> Pushing to Docker Hub..."
    exec "${SCRIPT_DIR}/docker-push.sh" "${VERSION}"
fi
