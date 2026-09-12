# Production checklist

NovaWM is usable as a daily driver when the items below are green.

## Compositor features

- [x] Tiling layouts (scroll / dwindle) + canvas
- [x] Multi-monitor + workspaces
- [x] Layer-shell (quickshell / bars)
- [x] Idle notify + inhibit (`hypridle` / `swayidle`)
- [x] Session lock (`swaylock` / `hyprlock`)
- [x] Screenshots + screen record (`novactl`)
- [x] Window rules (float / opacity / workspace / center)
- [x] IPC + `novactl`
- [x] Screencast protocols (`ext-image-copy-capture` + output sources) - install `xdg-desktop-portal-wlr`; see `docs/portals.md`
- [ ] Xwayland (optional, for legacy apps)

## Install (Arch)

```bash
makepkg -si                    # from the source tree, packages the branch
# then pick "NovaWM" in SDDM/GDM/LightDM
```

See `docs/install.md`.

## Your session stack

```kdl
// config.kdl
startup "foot" "mako" "hypridle"
// + "qs -p ~/your-rice" if you use quickshell

// keys.kdl
bind "mod+d" "spawn fuzzel"     // or wofi / rofi
bind "mod+l" "spawn swaylock"
bind "mod+shift+s" "spawn novactl screenshot area copy"
```

## Verify after upgrade

```bash
git pull && cargo build --release
# restart NovaWM from TTY
wayland-info | rg -i 'idle|session_lock|layer_shell'
swaylock          # lock works
novactl list-windows
```

## Known non-goals (by design)

- Built-in app launcher - use `spawn`
- Built-in idle daemon - use hypridle
- Built-in notification daemon - use mako/swaync/qs
