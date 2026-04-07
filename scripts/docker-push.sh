#!/usr/bin/env bash
# ── Push DarshJDB images to Docker Hub ───────────────────────────────
# Usage:
#   ./scripts/docker-push.sh              # pushes :latest only
#   ./scripts/docker-push.sh 0.1.0        # pushes :0.1.0, :0.1, and :latest
#
# Prerequisites:
#   docker login (must be authenticated to darshjme account)
# ─────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="darshjme/darshjdb"
VERSION="${1:-}"

# Verify we're logged in
if ! docker info 2>/dev/null | grep -q "Username"; then
    echo "ERROR: Not logged in to Docker Hub."
    echo "Run: docker login"
    exit 1
fi

# Verify the image exists locally
if ! docker image inspect "${REPO}:latest" &>/dev/null; then
    echo "ERROR: Image ${REPO}:latest not found locally."
    echo "Run: ./scripts/docker-build.sh first"
    exit 1
fi

echo "==> Pushing ${REPO}:latest"
docker push "${REPO}:latest"

if [[ -n "${VERSION}" ]]; then
    echo "==> Pushing ${REPO}:${VERSION}"
    docker push "${REPO}:${VERSION}"

    # Push major.minor tag if semver
    if [[ "${VERSION}" =~ ^([0-9]+\.[0-9]+)\.[0-9]+$ ]]; then
        MINOR_TAG="${BASH_REMATCH[1]}"
        echo "==> Pushing ${REPO}:${MINOR_TAG}"
        docker push "${REPO}:${MINOR_TAG}"
    fi
fi

echo ""
echo "==> Push complete."
echo "    https://hub.docker.com/r/${REPO}/tags"
