#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# build.sh — Build the lwk-server:dev-builder Docker image
#
# Usage:
#   ./build.sh --version <version>
#   ./build.sh --version <version> --override
#   ./build.sh --version <version> --reuse
#
# Options:
#   --version <ver>   Required. Semantic version for the image (e.g. 1.0.0).
#                     Produces: lwk-server:dev-builder-<ver>
#                               lwk-server:dev-builder  (alias used by test.sh)
#   --override        Allow overwriting an existing image with the same version.
#   --reuse           Reuse existing image if it exists (skip build entirely).
#                     Useful when only source code changed (mounted via volume).
#
# Examples:
#   ./build.sh --version 1.0.0
#   ./build.sh --version 1.0.0 --override
#   ./build.sh --version 1.0.0 --reuse
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

VERSION=""
OVERRIDE=false
REUSE=false

# ── Parse args ───────────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            if [[ -z "${2:-}" ]]; then
                echo "ERROR: --version requires a value (e.g. --version 1.0.0)"
                exit 1
            fi
            VERSION="$2"
            shift 2
            ;;
        --override)
            OVERRIDE=true
            shift
            ;;
        --reuse)
            REUSE=true
            shift
            ;;
        *)
            echo "ERROR: Unknown argument: $1"
            echo "Usage: ./build.sh --version <version> [--override|--reuse]"
            exit 1
            ;;
    esac
done

if [[ -z "$VERSION" ]]; then
    echo "ERROR: --version is required."
    echo "Usage: ./build.sh --version <version> [--override|--reuse]"
    exit 1
fi

VERSIONED_TAG="lwk-server:dev-builder-${VERSION}"
LATEST_TAG="lwk-server:dev-builder"

# ── Guard: check if versioned image already exists ───────────────────────

if docker image inspect "$VERSIONED_TAG" &>/dev/null; then
    if [[ "$REUSE" == true ]]; then
        echo "Image '$VERSIONED_TAG' already exists — reusing (skipping build)."
        docker tag "$VERSIONED_TAG" "$LATEST_TAG"
        echo ""
        echo "=== Done (reused) ==="
        echo "  Versioned : $VERSIONED_TAG"
        echo "  Alias     : $LATEST_TAG"
        echo ""
        echo "Run tests with: ./test.sh --build-version ${VERSION}"
        exit 0
    fi
    if [[ "$OVERRIDE" == false ]]; then
        echo "ERROR: Image '$VERSIONED_TAG' already exists."
        echo "Use --override to force a rebuild, or --reuse to skip build."
        exit 1
    fi
    echo "WARNING: Image '$VERSIONED_TAG' already exists — overriding."
    echo ""
fi

# ── Clear cargo cache ────────────────────────────────────────────────────────

CARGO_CACHE="${HOME}/.cargo-lwk-cache"
if [[ -d "$CARGO_CACHE" ]]; then
    echo "Clearing cargo cache: $CARGO_CACHE"
    rm -rf "$CARGO_CACHE"
    echo "Cache cleared."
    echo ""
fi

# ── Build ────────────────────────────────────────────────────────────────────

echo "=== Building $VERSIONED_TAG ==="
echo "Context: context/"
echo ""

docker build -t "$VERSIONED_TAG" context/

# Also tag as the unversioned alias used by test.sh
docker tag "$VERSIONED_TAG" "$LATEST_TAG"

echo ""
echo "=== Done ==="
echo "  Versioned : $VERSIONED_TAG"
echo "  Alias     : $LATEST_TAG"
echo ""
echo "Run tests with: ./test.sh --build-version ${VERSION}"
