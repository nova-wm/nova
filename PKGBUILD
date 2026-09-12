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
url="https://github.com/Frqme21/NovaWM"
license=('AGPL-3.0')
depends=(
  'gcc-libs' 'glibc' 'libxkbcommon' 'libinput' 'libdisplay-info'
  'libseat' 'mesa' 'pixman' 'wayland' 'libpipewire' 'systemd-libs'
)
optdepends=(
  'xdg-desktop-portal' 'xdg-desktop-portal-wlr' 'pipewire' 'wireplumber'
  'foot' 'mako' 'hypridle' 'swaylock' 'fuzzel' 'ffmpeg'
  'sddm' 'gdm' 'lightdm'
)
makedepends=(
  'rust' 'cargo' 'git' 'pkgconf' 'wayland-protocols'
  'libdrm' 'gbm' 'libglvnd' 'seatd' 'systemd'
)
provides=('wayland-compositor')
options=('!lto' '!strip')
# Dummy source so makepkg's extract step is happy; real build uses $startdir.
source=("$pkgname-$pkgver.localstub")
sha256sums=('SKIP')

prepare() {
  # Create the stub the source= line refers to (not a real tarball).
  : > "$srcdir/$pkgname-$pkgver.localstub"
}

build() {
  cd "$startdir"
  export CARGO_HOME="${CARGO_HOME:-$srcdir/cargo-home}"
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
