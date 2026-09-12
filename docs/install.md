# Installing NovaWM

## Arch Linux (recommended)

```bash
git clone https://github.com/nova-wm/nova
cd nova
makepkg -si
```

The shipped `PKGBUILD` builds directly from the working tree - no release
tarball, no downloads. It installs the `novawm` + `novactl` binaries, a
**Wayland session** desktop entry for every common display manager, the default
config, and docs.

## From source, no package manager

```bash
cargo build --release
sudo ./target/release/novawm drm   # from a TTY
```

NovaWM needs DRM access to show anything - run it from a TTY or a display
manager, not from inside another Wayland session. For fun, there's also a
nested backend: `./target/release/novawm winit`.

## Display managers

The file `/usr/share/wayland-sessions/novawm.desktop` is picked up by:

| DM | How to select |
|----|----------------|
| **SDDM** | Session menu → *NovaWM* |
| **GDM** | Gear → *NovaWM* |
| **LightDM** | greeter session list (Wayland-capable greeter) |
| **Ly / greetd** | choose `novawm` / `novawm-session` |

No greeter-specific config required.

### TTY

```bash
novawm-session
```

## First login

`~/.config/novawm/` is seeded from `/usr/share/novawm/config/` once.

## Screen share

```bash
sudo pacman -S xdg-desktop-portal xdg-desktop-portal-wlr pipewire wireplumber
mkdir -p ~/.config/xdg-desktop-portal
cp /usr/share/novawm/portals.conf ~/.config/xdg-desktop-portal/portals.conf
```

## Uninstall

```bash
sudo pacman -Rns novawm
```
