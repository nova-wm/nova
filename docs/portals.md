# Screen share / xdg-desktop-portal

NovaWM implements the compositor side of modern screen capture:

| Global | Purpose |
|--------|---------|
| `ext_output_image_capture_source_manager_v1` | Name an output as a capture source |
| `ext_image_copy_capture_manager_v1` | Client submits buffers; compositor fills them |

Together these are what **xdg-desktop-portal-wlr** (recent versions) and
**xdg-desktop-portal-generic** prefer for Discord / Zoom / browser share.

## What you still install (user session)

The compositor only exposes Wayland protocols. The portal stack turns that
into D-Bus + PipeWire for apps:

```bash
# Arch example
sudo pacman -S xdg-desktop-portal xdg-desktop-portal-wlr pipewire wireplumber

# Ensure the wlr portal is chosen (once)
mkdir -p ~/.config/xdg-desktop-portal
cat > ~/.config/xdg-desktop-portal/portals.conf <<'EOF'
[preferred]
default=wlr
org.freedesktop.impl.portal.ScreenCast=wlr
org.freedesktop.impl.portal.Screenshot=wlr
EOF
```

Autostart with the session (or let systemd user units handle it):

```kdl
// config.kdl - optional; many distros start portals via systemd
// startup "/usr/lib/xdg-desktop-portal -r"
// startup "/usr/lib/xdg-desktop-portal-wlr"
```

After login (from a terminal **on NovaWM**):

```bash
# NovaWM sets XDG_CURRENT_DESKTOP=wlroots itself on DRM startup.
# Still push the env into systemd user services:
systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP
dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP
systemctl --user restart xdg-desktop-portal xdg-desktop-portal-wlr
wayland-info | rg -i 'image_copy_capture|image_capture_source'
# should list the ext_* globals
```

Then try Discord / OBS / Firefox screen share.

## Built-in capture (no portal)

| Need | Tool |
|------|------|
| Screenshot | `novactl screenshot area\|monitor copy\|save` |
| Local record | `novactl record` / `mod+shift+r` |

## Limits (v1)

- **Output / monitor capture only.** Per-window share needs foreign-toplevel
  image sources (not wired yet). In Discord pick a **Screen**, not a window.
- **SHM buffers** (Argb/Abgr). No DMA-BUF export yet - fine for Vesktop/OBS,
  a bit more CPU than Hyprland's dmabuf path.
- Cursor-only capture sessions are rejected (cursor baked in when requested).
- Vesktop/Electron: launch with Wayland ozone if needed:
  `vesktop --ozone-platform=wayland`

## Troubleshooting

| Symptom | Check |
|---------|-------|
| "No screen capture sources" | portal-wlr running? `ext_*` globals present? |
| Black frames | rebuild on tip with `image-copy-capture` in feature banner |
| Works in novactl record but not Discord | portal stack, not the compositor capture path |
