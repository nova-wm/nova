# Keybinds

Keybinds live in `keys.kdl` (main) and `canvas_keys.kdl` (canvas-only), both
`include`d from `config.kdl`. `mod` means whatever `modifier "<key>"` is set
to in `config.kdl` - `"super"` by default.

Binds are `bind "<combo>" "<action>"`. Mouse-drag binds use `bindm`.

## Window management

| Bind | Action |
|------|--------|
| `mod+Return` | Spawn terminal (the configured `terminal`, `foot` by default) |
| `mod+c` | Close focused window |
| `mod+f` | Fullscreen / fill |
| `mod+v` | Toggle float |
| `mod+grave` | Toggle scratchpad |
| `mod+o` | Layout overview (overlay picking a window) |
| `mod+Shift+r` | Toggle screen recording |
| `mod+mouse:272` drag | Move window |
| `mod+mouse:273` drag | Resize window |

## Focus & move

| Bind | Action |
|------|--------|
| `mod+h` / `mod+j` / `mod+k` / `mod+l` | Focus left / down / up / right |
| `mod+Shift+h` | Move focused window to previous position |
| `mod+Shift+l` | Move focused window to next position |
| `mod+Shift+j` / `mod+Shift+k` | Move window to next / previous row |
| `mod+comma` | Consume window into the column to the left |
| `mod+period` | Expel window from its column |
| `mod+Tab` / `mod+Shift+Tab` | Cycle focused window forward / backward |

## Workspaces

| Bind | Action |
|------|--------|
| `mod+1` … `mod+9` | Switch to workspace 1–9 |

## Canvas (when `layout "canvas"` is active)

| Bind | Action |
|------|--------|
| `mod+equal` / `mod+minus` | Zoom in / out around the cursor |
| `mod+0` | Reset zoom to 1.0, no pan |
| `mod+x` | Center the view on the focused window |
| `mod+w` | Zoom to fit all windows |
| `mod+Shift+Arrows` | Nudge the focused window |
| `mod+Ctrl+Arrows` | Pan the canvas |
| `mod+t` | Pin / unpin the focused window to its screen spot |
| `mod+a` | Home - reset the canvas view |
| `mod+scroll` | Zoom around the cursor |
| `mod+Shift+scroll` | Switch workspaces |
| `mod+drag` on empty space | Pan |
| `mod+drag` a window | Move it |
| `mod+right-drag` a window | Resize it freely |

## Editing

Un-comment the launcher / lock / screenshot binds you want in `keys.kdl`:

```kdl
bind "mod+d" "spawn fuzzel"
bind "mod+shift+s" "spawn novactl screenshot area copy"
bind "mod+l" "spawn swaylock"
```

`spawn` runs through `sh -c`, so `~`, `$VAR`, pipes, and flags all work.