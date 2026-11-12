#!/usr/bin/env bash
# ci-build.sh - build the Arch package from this checkout, inside an
# archlinux container as root (release.yml; locally:
#   docker run --rm -v "$PWD:/src" -w /src archlinux:latest packaging/aur/ci-build.sh
# ). Leaves timehud-<ver>-<rel>-x86_64.pkg.tar.zst in dist/ and its name in
# $GITHUB_ENV as ARCHPKG when set.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

pacman -Syu --noconfirm --needed base-devel git namcap

# shellcheck source=PKGBUILD disable=SC1091
source packaging/aur/PKGBUILD
CRATE=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
if [ "$pkgver" != "$CRATE" ]; then
    echo "::error::PKGBUILD pkgver $pkgver does not match Cargo.toml version $CRATE"
    exit 1
fi
pacman -S --noconfirm --needed "${depends[@]}" "${makedepends[@]}"

# makepkg refuses to run as root.
id builder >/dev/null 2>&1 || useradd -m builder
WORK=$(mktemp -d)
cp packaging/aur/PKGBUILD "$WORK/"
git config --global --add safe.directory "$ROOT"
# Stands in for the GitHub tag archive the PKGBUILD would download.
git archive --prefix="$pkgname-$pkgver/" -o "$WORK/$pkgname-$pkgver.tar.gz" HEAD
chown -R builder: "$WORK"
su builder -c "cd '$WORK' && makepkg --noconfirm"

PKG="$pkgname-$pkgver-$pkgrel-x86_64.pkg.tar.zst"
namcap "$WORK/PKGBUILD"
namcap "$WORK/$PKG" | tee "$WORK/namcap.txt"
if grep -q ' E: ' "$WORK/namcap.txt"; then
    echo "::error::namcap reported errors"
    exit 1
fi

mkdir -p dist
cp "$WORK/$PKG" dist/
echo "built dist/$PKG"
if [ -n "${GITHUB_ENV:-}" ]; then
    echo "ARCHPKG=dist/$PKG" >> "$GITHUB_ENV"
fi
