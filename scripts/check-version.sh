#!/bin/sh
# Verify that Cargo, AppLoad, changelog, and release tag versions agree.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)
TAG="${1:-}"
if [ -z "$TAG" ]; then
    case "${GITHUB_REF_NAME:-}" in
        v*) TAG="$GITHUB_REF_NAME" ;;
        *) TAG="v$VERSION" ;;
    esac
fi
MANIFEST_VERSION=$(
    sed -n 's/^[[:space:]]*"version": "\(.*\)",/\1/p' \
        "$ROOT/packaging/appload/external.manifest.json" |
        head -n 1
)

if ! printf '%s\n' "$VERSION" |
    grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'; then
    echo "Cargo.toml contains an invalid semantic version: $VERSION" >&2
    exit 1
fi

if [ "$TAG" != "v$VERSION" ]; then
    echo "tag $TAG does not match Cargo version $VERSION" >&2
    exit 1
fi

if [ "$MANIFEST_VERSION" != "$VERSION" ]; then
    echo "AppLoad version $MANIFEST_VERSION does not match Cargo version $VERSION" >&2
    exit 1
fi

if ! grep -Fq "## [$VERSION] -" "$ROOT/CHANGELOG.md"; then
    echo "CHANGELOG.md has no dated section for $VERSION" >&2
    exit 1
fi

printf 'version %s is consistent\n' "$VERSION"
