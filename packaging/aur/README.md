# AUR packaging

`PKGBUILD` + `.SRCINFO` for the [AUR](https://aur.archlinux.org).

## Test locally

```bash
cd packaging/aur
makepkg -si        # builds from the GitHub tag tarball and installs
```

## Publish / update on the AUR

One-time: create an AUR account and add your SSH key at
https://aur.archlinux.org/account.

```bash
git clone ssh://aur@aur.archlinux.org/timehud.git aur-timehud
cp packaging/aur/PKGBUILD packaging/aur/.SRCINFO aur-timehud/
cd aur-timehud && git add -A && git commit -m "0.5.0" && git push
```

## New release checklist

1. Tag `vX.Y.Z` on GitHub (also builds the AppImage).
2. `pkgver=X.Y.Z`, `pkgrel=1` in PKGBUILD.
3. `updpkgsums` (refreshes sha256sums), `makepkg --printsrcinfo > .SRCINFO`.
4. Commit here and push to the AUR repo.
