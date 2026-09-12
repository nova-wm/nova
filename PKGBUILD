# NovaWM PKGBUILD.
#
#   cd /path/to/NovaWM
#   makepkg -si
#
# Builds from the current working tree; no release tarball needed.

pkgname=novawm
pkgver=0.1.0
pkgrel=1
pkgdesc="Tiling Wayland compositor (scroll / dwindle / canvas)"
arch=('x86_64' 'aarch64')
url="https://github.com/nova-wm/nova"
license=('AGPL-3.0')
depends=(
  'gcc-libs' 'glibc' 'libxkbcommon' 'libinput' 'libdisplay-info'
  'seatd' 'mesa' 'pixman' 'wayland' 'libpipewire' 'systemd-libs'
)
optdepends=(
  'xdg-desktop-portal' 'xdg-desktop-portal-wlr' 'pipewire' 'wireplumber'
  'foot' 'mako' 'hypridle' 'swaylock' 'fuzzel' 'ffmpeg'
  'sddm' 'gdm' 'lightdm'
)
makedepends=(
  'rust' 'cargo' 'git' 'pkgconf' 'wayland-protocols'
  'libdrm' 'seatd' 'systemd'
)
provides=('wayland-compositor')
options=('!lto' '!strip')
# Build from the directory this PKGBUILD lives in - no remote sources.
source=()
sha256sums=()

prepare() {
  export CARGO_HOME="${CARGO_HOME:-$srcdir/cargo-home}"
}

build() {
  cd "$startdir"
  cargo build --release
}

package() {
  cd "$startdir"

  install -Dm755 target/release/novawm  "$pkgdir/usr/bin/novawm"
  install -Dm755 target/release/novactl "$pkgdir/usr/bin/novactl"

  install -Dm755 packaging/usr/lib/novawm/novawm-session \
    "$pkgdir/usr/lib/novawm/novawm-session"
  install -Dm755 packaging/usr/bin-novawm-session \
    "$pkgdir/usr/bin/novawm-session"

  install -Dm644 packaging/usr/share/wayland-sessions/novawm.desktop \
    "$pkgdir/usr/share/wayland-sessions/novawm.desktop"

  install -d "$pkgdir/usr/share/novawm/config"
  install -Dm644 packaging/usr/share/novawm/config/* \
    "$pkgdir/usr/share/novawm/config/"

  install -Dm644 packaging/usr/share/novawm/portals.conf \
    "$pkgdir/usr/share/novawm/portals.conf"

  install -d "$pkgdir/usr/share/doc/novawm"
  install -Dm644 docs/*.md "$pkgdir/usr/share/doc/novawm/" 2>/dev/null || true

  if [ -f LICENSE ]; then
    install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  fi
}
