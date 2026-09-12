# Layouts

NovaWM has three layout engines. The active one is set with `layout "<name>"`
in `config.kdl`, or at runtime with `novactl layout set <scroll|dwindle|canvas>`.

## Scroll (`layout "scrolling"`)

A niri-style floating strip. Windows become columns side by side, each
spanning the full working-area height:

```
┌──────┬──────┬──────┬───┐
│  A   │  B   │  C   │ D │   ← focused column B centered on screen
│      │      │      │   │
└──────┴──────┴──────┴───┘
```

- The view is *recomputed* from the focused column, never accumulated, so it
  can't drift.
- Per-window column widths: grab a tiled window's edge while holding `mod` and
  drag - that column grows and pushes its neighbors along the strip.
- `mod+comma` / `mod+period` consume into / expel from a column.
- `mod+Shift+j` / `mod+Shift+k` move a window to the next/previous row.
- Maximizing one window (`mod+f`) makes it full-width; the rest are pushed
  aside and revealed when you scroll focus off it.
- The strip is one continuous virtual space that wraps across all monitors -
  focus changes drive which workspace is where via the scroll strip.

## Dwindle (`layout "dwindle"`)

A binary split tree, like i3 / Hyprland default:

```
        ┌──────┬──────┐
        │  A   │  B   │
        ├──────┤      │
        │  C   │      │
        └──────┴──────┘
```

- Every window is a leaf of a split tree; new windows split the focused leaf.
- Windows are laid out recursively from the root; gaps come from config.
- Remove a window and the split that held it collapses back parent way.

## Canvas (`layout "canvas"`)

An infinite, zoomable plane:

```
   (zoom out)             (zoom in on a window)
 ┌┐  ┌─┐ ┌──┐
 └┘  └─┘ └──┘   ← full workspace, tiny
                          ┌──────────────┐
                          │ focused here │   ← camera zooms in, clients NOT resized
                          └──────────────┘
```

- **Free placement.** Windows sit at arbitrary rects; nothing auto-tiles.
- **True view-zoom.** The camera scales the render only - clients are never
  told they got resized.
- `mod+scroll` zooms around the cursor; `mod+drag` on empty space pans;
  `mod+drag` a window moves it; `mod+right-drag` resizes freely.
- Keys: `mod+equal`/`mod+minus` zoom, `mod+x` center focused, `mod+w` fit all,
  `mod+t` pin, `mod+a` home - see `canvas_keys.kdl` / `docs/keys.md`.
- The canvas is always one camera; no workspaces. Pinned windows keep their
  screen spot regardless of pan/zoom.

## Tuning

- `scroll.kdl` - `scroll { speed 400 }`, scrolling animation speed.
- `dwindle.kdl` - `dwindle { split_ratio 0.5, split_direction "auto" }`
  (ratio of a new split given to the first window; direction auto/horizontal/
  vertical).
- `canvas_keys.kdl` - canvas keybinds (only read when canvas is active).