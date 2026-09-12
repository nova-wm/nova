# Quickshell on NovaWM

NovaWM implements the Wayland protocols desktop-shell clients need, so
[quickshell](https://quickshell.org/) panels, bars, widgets, launchers,
notifications and OSDs work out of the box - on both the nested (`winit`)
backend and the real DRM backend.

## Setup

1. Install quickshell from your distro (or build it with Wayland support).
2. Autostart it from `config.kdl`:

   ```kdl
   startup "foot" "quickshell"
   ```

   (Multiple `startup` entries all launch at startup. Or run `quickshell`
   from a terminal inside the session for logs.)
3. Write your shell the usual way - prefer the portable `PanelWindow`
   component; it picks layer-shell automatically on NovaWM.

## What is supported

| Feature | Protocol | Notes |
|---|---|---|
| Panels / bars / widgets / OSDs | `zwlr_layer_shell_v1` | All four layers; anchored, full-width bars work |
| Exclusive zones | (part of layer-shell) | Tiles shrink around bars; freed space reflows when a panel closes |
| Launcher / lock-screen keyboard capture | (layer-shell interactivity) | `Exclusive` Top/Overlay layers get every key, including compositor shortcuts |
| Click-to-focus on panels | - | Focusable layers focus on click; `None`-focus bars stay click-through for typing |
| Popups on panels | `xdg_popup` on layer parents | Menus/tooltips anchored to a bar constrain to its output |
| HiDPI clients | `wp_fractional_scale_v1` + `wp_viewporter` | Preferred scale is advertised per output |
| Solid-color buffers | `wp_single_pixel_buffer_v1` | Used by Qt for flat fills |
| Idle inhibition | `zwp_idle_inhibit_manager_v1` | Tracked per surface |
| Multi-monitor panels | `zxdg_output_v1` + per-output layer maps | Panels bind to the screen quickshell requests; on hotplug-disconnect they migrate to a surviving output |

## Debugging

- `novactl layers` lists every mapped layer surface: output, namespace,
  layer, whether it can take keyboard focus, and geometry.
- `novactl monitors` shows output geometries; tiles avoid the exclusive
  zones reported there.
- The compositor log (`~/.local/state/novawm/novawm-life.log`) records
  layer map/unmap/configure events with the `layer-shell:` prefix.

## Known limitations (not yet implemented)

- **Taskbars / window lists** (`zwp… ToplevelManager`): quickshell's
  `Toplevel`/`ToplevelManager` types need
  `zwlr_foreign_toplevel_management_v1`, which Smithay does not provide -
  only the newer `ext_foreign_toplevel_list_v1` exists upstream. Window
  listing stays available through `novactl list-windows` instead.
- **Screencopy portal** (xdg-desktop-portal screencast): still external;
  built-in `novactl screenshot` / `record` cover local capture. Portal
  wiring is a separate backlog item.
- **Session lock + idle**: implemented (`ext_session_lock_v1`,
  `ext_idle_notifier_v1` + inhibit). Use swaylock/hyprlock + hypridle;
  see `docs/session.md`. An Exclusive layer-shell overlay is still *not*
  a secure lock - use the real lock protocol.
