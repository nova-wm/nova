# Window rules

Match newly mapped windows by `app_id` and/or `title` and apply actions.

## Syntax

```kdl
// config.kdl (or any include)
windowrule {
    app_id "mpv"          // substring, case-insensitive; omit = any
    title "Picture"       // substring, case-insensitive; omit = any
    float                 // start floating (same as Super+v)
    center                // center on the output (with float)
    opacity 1.0           // 0.0–1.0, overrides decorations.opacity
    workspace 3           // 1–9, pins the window and jumps there
}
```

Multiple `windowrule` nodes accumulate. Later matching rules override earlier
ones for the same action (last opacity / workspace wins; float/center OR).

## Discover app_id

```bash
novactl list-windows
# index  app_id  title  output  workspace  floating  focused  geometry
```

Common ids: `firefox`, `chromium`, `org.telegram.desktop`, `discord`,
`spotify`, `org.gnome.Nautilus`, `mpv`, `org.pwmt.zathura`.

## Examples

```kdl
// Media always opaque + floating
windowrule {
    app_id "mpv"
    float
    opacity 1.0
}

// Firefox PiP
windowrule {
    title "Picture-in-Picture"
    float
    center
}

// Chat on workspace 3
windowrule {
    app_id "discord"
    workspace 3
}
windowrule {
    app_id "telegram"
    workspace 3
}

// File picker-ish dialogs (title match)
windowrule {
    title "Open File"
    float
    center
}
```

## When rules apply

On first map and again on later commits until `app_id`/`title` arrive (many
clients set them after the first frame). Float / workspace / center run once
per window; opacity can refresh if a later commit changes identity.

## Not yet

- `size w h` / `move x y`
- regex matchers
- one-shot vs permanent
- layer-shell rules
