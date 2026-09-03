# AUR packaging (ferriscribe-bin)

Prebuilt package for Arch-based systems (including Omarchy): repackages the
official release `.deb` from GitHub Releases. Nothing is compiled — `makepkg`
extracts the deb and renames the binary so `Exec=` resolves.

## Install (end users)

```sh
yay -S ferriscribe-bin        # or: paru -S ferriscribe-bin
```

Runtime deps (`webkit2gtk-4.1`, `gtk3`, `alsa-lib`, `libayatana-appindicator`)
come from Arch's official repos.

## Publishing / updating (maintainer)

One-time setup: clone the AUR repo over ssh
(`git clone ssh://aur@aur.archlinux.org/ferriscribe-bin.git`) and copy
`PKGBUILD` into it.

Per release (after the tag's Release workflow finishes):

```sh
# 1. Bump pkgver in PKGBUILD to the released version.
# 2. Refresh the checksum:
curl -sL -o /tmp/fs.deb "https://github.com/cortexuvula/ferriscribe/releases/download/v<VERSION>/FerriScribe_<VERSION>_amd64.deb"
shasum -a 256 /tmp/fs.deb   # paste into sha256sums
# 3. Regenerate .SRCINFO (AUR requires it committed):
makepkg --printsrcinfo > .SRCINFO
# 4. Commit + push to the AUR repo.
```

## Notes

- Auto-updates: the in-app updater on Linux only works for AppImage installs
  (also shipped per release). The pacman package updates via `pacman -Syu`
  when this AUR package is refreshed.
- The binary and desktop entry ship as `rust-medical-assistant` inside the
  deb (the Cargo package name); `package()` renames the binary to
  `ferriscribe` and patches only the `Exec=` line, keeping `Icon=` /
  `StartupWMClass=` consistent with the installed icon files and the app's
  window-manager class.
