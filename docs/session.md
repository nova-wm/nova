# Daily-driver session on NovaWM

NovaWM is the compositor. Everything around it - launcher, notifications,
idle, lock - is your choice of external tools. Bind and autostart them;
NovaWM speaks the protocols they need.

## App launcher

Any launcher works. Bind it with `spawn` (runs through `sh -c`):

```kdl
// keys.kdl
bind "mod+d" "spawn fuzzel"
// or: spawn wofi --show drun
// or: spawn rofi -show drun
// or: spawn qs -c ~/your-rice -i launcher
```

## Notifications

```kdl
// config.kdl
startup "mako"
// or: startup "swaync"
```

Layer-shell panels from quickshell can also own notifications.

## Idle + lock (hypridle / swayidle)

NovaWM implements:

- `zwp_idle_inhibit_manager_v1` - media players / shell can block idle
- `ext_idle_notifier_v1` - hypridle / swayidle timers
- `ext_session_lock_v1` - swaylock / hyprlock / qs lock screens

Example `~/.config/hypr/hypridle.conf`:

```conf
general {
    lock_cmd = pidof swaylock || swaylock
    before_sleep_cmd = loginctl lock-session
}

listener {
    timeout = 300
    on-timeout = loginctl lock-session
}
listener {
    timeout = 600
    on-timeout = systemctl suspend
}
```

Autostart:

```kdl
startup "hypridle"
// and bind a manual lock:
// keys.kdl: bind "mod+l" "spawn swaylock"
```

`swaylock` / `hyprlock` use `ext-session-lock`. While locked, NovaWM:

- routes keyboard + pointer only to the lock surface
- draws the lock surface full-output above the session
- suppresses compositor shortcuts and window grabs

## Screenshots / record

Built in - no external tool required:

```kdl
bind "mod+shift+s" "spawn novactl screenshot area copy"
bind "mod+shift+r" "toggle_record"
```

## Full example startup

```kdl
startup "foot" \
        "qs -p ~/nothing-rice-new/quickshell" \
        "mako" \
        "hypridle"
```

## What NovaWM does *not* ship

| Want | Use |
|------|-----|
| App launcher | fuzzel / wofi / rofi / qs |
| Notifications | mako / swaync / qs |
| Idle daemon | hypridle / swayidle |
| Lock screen | swaylock / hyprlock / qs |
| Logout menu | wlogout / qs |
| Wallpaper daemon | built-in `background` path, or swaybg |
| Portals / screencast | xdg-desktop-portal-wlr (future) |


## Window rules

```kdl
windowrule {
    app_id "mpv"
    float
    opacity 1.0
}
```

See `docs/window-rules.md`. Discover ids with `novactl list-windows`.

## Screen share (Discord / Zoom / browser)

Compositor protocols are implemented. Install the portal stack:

```bash
# + xdg-desktop-portal + xdg-desktop-portal-wlr + pipewire
```

See `docs/portals.md`.
