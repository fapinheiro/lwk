#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# test.sh — LWK unit tests + sponsor golden fixture generation
#
# Usage:
#   ./test.sh --build-version <version>
#   ./test.sh --build-version <version> --fixtures-only
#
# Options:
#   --build-version <ver>   Required. Version of the Docker image to use.
#                           The image must have been built with:
#                             ./build.sh --version <ver>
#                           Expects: lwk-server:dev-builder-<ver>
#
#   --fixtures-only         Skip Step 1 (unit tests) and go straight to
#                           fixture generation (Step 2). Use when unit tests
#                           are already known to pass and you only need to
#                           regenerate the golden fixtures.
#
# Examples:
#   ./test.sh --build-version 1.0.0
#   ./test.sh --build-version 1.0.0 --fixtures-only
#
# What it does:
#   1. [unit tests]   Runs lwk_wollet unit tests (cargo test --lib).
#                     Skipped with --fixtures-only.
#   2. [fixtures]     Runs test_generate_sponsor_fixtures (--ignored) inside
#                     Docker. Starts elementsd + electrs in regtest mode,
#                     executes a full sponsor flow, and writes JSON/base64
#                     fixtures to lwk_wollet/tests/data/sponsor_fixtures/.
#   3. [validation]   Checks that all expected fixture files were created.
#
# Prerequisites:
#   - Docker image built:  ./build.sh --version <ver>
#   - Docker daemon running locally
#
# After running, check for changed fixture files:
#   git diff lwk_wollet/tests/data/sponsor_fixtures/
#
# Then copy the fixtures to the other repos:
#   tappix-api/src/test/resources/fixtures/sponsor/
#   tappix-ios/TappixAppTests/Fixtures/sponsor/
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

CARGO="cargo"
FIXTURES_DIR="lwk_wollet/tests/data/sponsor_fixtures"
FIXTURES_ONLY=false
BUILD_VERSION=""

# ── Parse args ───────────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --build-version)
            if [[ -z "${2:-}" ]]; then
                echo "ERROR: --build-version requires a value (e.g. --build-version 1.0.0)"
                exit 1
            fi
            BUILD_VERSION="$2"
            shift 2
            ;;
        --fixtures-only)
            FIXTURES_ONLY=true
            shift
            ;;
        *)
            echo "ERROR: Unknown argument: $1"
            echo "Usage: ./test.sh --build-version <version> [--fixtures-only]"
            exit 1
            ;;
    esac
done

if [[ -z "$BUILD_VERSION" ]]; then
    echo "ERROR: --build-version is required."
    echo "Usage: ./test.sh --build-version <version> [--fixtures-only]"
    echo "Build the image first with: ./build.sh --version <version>"
    exit 1
fi

DOCKER_IMAGE="lwk-server:dev-builder-${BUILD_VERSION}"

# ── Pre-flight ───────────────────────────────────────────────────────────────

if ! docker image inspect "$DOCKER_IMAGE" &>/dev/null; then
    echo "ERROR: Docker image '$DOCKER_IMAGE' not found."
    echo "Build it first with: ./build.sh --version ${BUILD_VERSION}"
    exit 1
fi

echo "=== LWK Test Runner ==="
echo "Docker image: $DOCKER_IMAGE"
echo ""

# Common docker run args:
# - Mount the repo as /workspace
# - Mount host cargo cache to avoid re-downloading crates on every run
# - Source env.sh to set ELEMENTSD_EXEC, ELECTRS_LIQUID_EXEC, etc.
# - The binaries (elementsd, electrs) are already inside the image at /bin/
CARGO_CACHE="${HOME}/.cargo-lwk-cache"
mkdir -p "$CARGO_CACHE/registry" "$CARGO_CACHE/git"
DOCKER_RUN="docker run --rm \
  -v $(pwd):/workspace \
  -v ${CARGO_CACHE}/registry:/usr/local/cargo/registry \
  -v ${CARGO_CACHE}/git:/usr/local/cargo/git \
  -w /workspace \
  $DOCKER_IMAGE"

# ── Step 1: Unit tests ───────────────────────────────────────────────────────

if [[ "$FIXTURES_ONLY" == false ]]; then
    echo "── Step 1/3: Running lwk_wollet unit tests ──"
    $DOCKER_RUN bash -c "source context/env.sh && $CARGO test -p lwk_wollet --lib"
    UNIT_EXIT=$?

    if [[ $UNIT_EXIT -ne 0 ]]; then
        echo ""
        echo "FAILED: Unit tests failed with exit code $UNIT_EXIT"
        exit $UNIT_EXIT
    fi
    echo ""
    echo "Unit tests: PASSED"
    echo ""
else
    echo "── Skipping unit tests (--fixtures-only) ──"
    echo ""
fi

# ── Step 2: Generate sponsor fixtures ────────────────────────────────────────

echo "── Step 2/3: Generating sponsor golden fixtures ──"
echo "This starts elementsd + electrs in regtest mode inside Docker..."
echo ""

$DOCKER_RUN bash -c "source context/env.sh && $CARGO test -p lwk_wollet --test e2e -- test_generate_sponsor_fixtures --ignored --nocapture"
FIXTURES_EXIT=$?

if [[ $FIXTURES_EXIT -ne 0 ]]; then
    echo ""
    echo "FAILED: Fixture generation failed with exit code $FIXTURES_EXIT"
    exit $FIXTURES_EXIT
fi

echo ""
echo "Fixture generation: PASSED"
echo ""

# ── Step 3: Validate output ─────────────────────────────────────────────────

echo "── Step 3/3: Validating generated fixtures ──"

EXPECTED_FILES=(
    "metadata.json"
    "utxo-secrets.json"
    "pset-unsigned.base64"
    "pset-prepared.json"
    "outputs.json"
    "pset-signed.base64"
    "pset-details.json"
    "errors/sponsor-sighash-not-all.json"
)

MISSING=0
for f in "${EXPECTED_FILES[@]}"; do
    if [[ ! -f "$FIXTURES_DIR/$f" ]]; then
        echo "  MISSING: $FIXTURES_DIR/$f"
        MISSING=$((MISSING + 1))
    else
        echo "  OK: $FIXTURES_DIR/$f"
    fi
done

echo ""

if [[ $MISSING -gt 0 ]]; then
    echo "FAILED: $MISSING expected fixture file(s) missing."
    exit 1
fi

# Validate metadata version
VERSION=$(python3 -c "import json; print(json.load(open('$FIXTURES_DIR/metadata.json'))['version'])" 2>/dev/null || echo "UNKNOWN")
echo "Fixture version: $VERSION"

echo ""
echo "=== ALL PASSED ==="
echo ""
echo "Generated fixtures in: $FIXTURES_DIR/"
echo ""
echo "Next steps:"
echo "  1. Check changes:  git diff $FIXTURES_DIR/"
echo "  2. Copy to backend: cp -r $FIXTURES_DIR/* ../tappix/tappix-api/src/test/resources/fixtures/sponsor/"
echo "  3. Copy to iOS:     cp -r $FIXTURES_DIR/* ../tappix/tappix-ios/TappixAppTests/Fixtures/sponsor/"
echo "  4. Run tests in each repo to validate compatibility"
