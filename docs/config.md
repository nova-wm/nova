# Config

NovaWM reads [KDL](https://kdl.dev) config files. On first login the shipped
config is copied to `~/.config/novawm/`; edit that. Also read from
`./config.kdl` if you run from a source tree.

`config.kdl` is the master file. Everything else is `include`d - the split
keeps related settings together:

| File | Contents |
|------|----------|
| `config.kdl` | Layout, gaps, monitors, animations, focus, decorations, startup |
| `keys.kdl` | Keybinds (`bind`) and drag binds (`bindm`) |
| `scroll.kdl` | Scroll-layout tuning (`speed`) |
| `dwindle.kdl` | Dwindle-layout tuning (`split_ratio`, `split_direction`) |
| `canvas_keys.kdl` | Canvas-only keybinds |

Includes are merged **after** the master file, so their values win.

## Top-level

```kdl
modifier "super"          // mod = Super. Also try "mod4", "ctrl", "alt"
layout "scrolling"        // "scrolling" | "dwindle" | "canvas"
terminal "foot"           // used by mod+Return and the scratchpad
welcome #true             // first-run tour window; reopen with mod+F1
```

## Monitors

```kdl
monitor "eDP-1"   { position 0 0 }
monitor "HDMI-A-1" { position 2560 0 }
```

Names come from `novactl monitors` (or `wlr-randr`).

## Gaps

```kdl
gaps {
    inner 20
    outer 20
}
```

## Animations

```kdl
animations {
    enabled #true
    duration 300          // ms
    bezier 0.25 0.1 0.25 1.0   // Hyprland-style cubic bezier
    open "scale_up"       // window-open animation
}
```

## Focus

```kdl
focus {
    follows_mouse #true
}
```

## Decorations

```kdl
decorations {
    rounding 12
    border_width 2
    active_border_color "#ff007f"
    inactive_border_color "#444444"
    border_effect "none"        // none | breathing | rainbow | rainbow_solid
                                // | gradient | flow | glow | pulse | ember | chase
                                // | bg | bg-N (N = 2..8)
    border_effect_speed 1.0
    border_gradient_color "#00d9ff"
    blur #true
    opacity 0.9
    focus_opacity 0.9
    grain #true
    grain_intensity 0.05
}
```

## Background

```kdl
background "/path/to/wallpaper.jpg"
```

## Startup

```kdl
startup "foot" \
        "qs -p ~/nothing-rice-new/quickshell" \
        "mako" \
        "hypridle"
```

Each quoted string is **one** shell command (run via `sh -c`), so `~`, `$VAR`,
pipes, and flags work. Separate apps = separate strings.

## Input

```kdl
input {
    focus-follows-mouse

    keyboard {
        xkb {
            layout "us,il"
            options "grp:alt_shift_toggle"
        }
    }

    touchpad {
        tap
        natural-scroll
        accel-profile "adaptive"
        accel-speed 0.2
        click-method "clickfinger"
        disable-while-typing
    }

    mouse {
        accel-profile "flat"
        accel-speed 0.2
    }
}
```

The xkb keymap applies at startup (restart after editing); touchpad/mouse
settings re-apply live on config reload.

## Window rules

Match newly mapped windows by `app_id` / `title` and apply actions.

```kdl
windowrule {
    app_id "mpv"
    float
    opacity 1.0
}
windowrule {
    title "Picture-in-Picture"
    float
    center
}
windowrule {
    app_id "discord"
    workspace 3
}
```

See [docs/window-rules.md](window-rules.md). Discover ids with
`novactl list-windows`.

## Editing live

NovaWM watches the config directory and reloads `.kdl` files on save. Layout
switches and most display settings apply immediately; the xkb map hasn't, yet.

## Where the files go

| Install | Path |
|---------|------|
| Arch package | `/usr/share/novawm/config/` (seed) |
| User | `~/.config/novawm/` (after first login) |
| Source tree | `./config.kdl`, `./keys.kdl`, … |