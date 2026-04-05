#!/bin/bash
set -euo pipefail

# DarshanDB npm publish script
# Builds and publishes all packages in dependency order:
#   1. @darshjdb/client   (client-core, no internal deps)
#   2. @darshjdb/react    (depends on @darshjdb/client)
#   3. @darshjdb/angular  (depends on @darshjdb/client)
#   4. @darshjdb/nextjs   (depends on @darshjdb/client + @darshjdb/react)

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

DRY_RUN=""
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN="--dry-run"
  echo "==> Dry run mode enabled"
fi

VERSION="${NPM_PUBLISH_VERSION:-}"
if [[ -n "$VERSION" ]]; then
  echo "==> Publishing version: $VERSION"
fi

echo ""
echo "========================================"
echo "  DarshanDB npm publish"
echo "========================================"
echo ""

# Build all packages in dependency order
PACKAGES=(
  "client-core"
  "react"
  "angular"
  "nextjs"
)

PACKAGE_NAMES=(
  "@darshjdb/client"
  "@darshjdb/react"
  "@darshjdb/angular"
  "@darshjdb/nextjs"
)

echo "==> Installing dependencies..."
cd "$ROOT_DIR"
npm install --ignore-scripts

echo ""
echo "==> Building packages..."
for pkg in "${PACKAGES[@]}"; do
  echo "  Building packages/$pkg..."
  cd "$ROOT_DIR/packages/$pkg"
  npx tsup
done

echo ""
echo "==> Running publish preflight checks..."
for i in "${!PACKAGES[@]}"; do
  pkg="${PACKAGES[$i]}"
  name="${PACKAGE_NAMES[$i]}"
  pkg_dir="$ROOT_DIR/packages/$pkg"

  if [[ ! -d "$pkg_dir/dist" ]]; then
    echo "ERROR: $pkg_dir/dist does not exist. Build failed for $name."
    exit 1
  fi

  echo "  $name: dist/ exists, ready to publish"
done

echo ""
echo "==> Publishing packages..."
for i in "${!PACKAGES[@]}"; do
  pkg="${PACKAGES[$i]}"
  name="${PACKAGE_NAMES[$i]}"
  cd "$ROOT_DIR/packages/$pkg"

  echo "  Publishing $name..."
  npm publish --access public $DRY_RUN

  if [[ -z "$DRY_RUN" ]]; then
    echo "  Published $name successfully"
  else
    echo "  Dry run complete for $name"
  fi
done

echo ""
echo "========================================"
echo "  All packages published successfully"
echo "========================================"
