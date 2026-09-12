//! Layer-shell support (`zwlr_layer_shell_v1`).
//!

use smithay::{
    desktop::{
        LayerSurface as DesktopLayerSurface, PopupKind, WindowSurfaceType,
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output,
    },
    output::Output,
    reexports::wayland_server::{
        protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        DisplayHandle,
    },
    utils::SERIAL_COUNTER,
    wayland::{
        compositor::{get_role, with_states},
        shell::{
            wlr_layer::{
                LAYER_SURFACE_ROLE, Layer as WlrLayer, LayerSurface as WlrLayerSurface,
                LayerSurfaceCachedState, LayerSurfaceData, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::PopupSurface,
        },
    },
};

use crate::Smallvil;

impl WlrLayerShellHandler for Smallvil {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        _layer: WlrLayer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.output_under(Some(self.cursor_position)))
            .or_else(|| self.space.outputs().next().cloned());

        let Some(output) = output else {
            tracing::warn!(namespace, "layer-shell: no output to place surface on");
            return;
        };

        let layer = DesktopLayerSurface::new(surface, namespace.clone());
        let mut map = layer_map_for_output(&output);
        if let Err(err) = map.map_layer(&layer) {
            tracing::warn!(?err, namespace, "layer-shell: failed to map layer surface");
            return;
        }
        crate::life(format!(
            "layer-shell: mapped {namespace} on {}",
            output.name()
        ));
        drop(map);
        self.arrange_windows();
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let wl_surface = surface.wl_surface().clone();
        if let Some((mut map, layer)) = self.space.outputs().find_map(|o| {
            let map = layer_map_for_output(o);
            let layer = map
                .layers()
                .find(|l| l.layer_surface() == &surface)
                .cloned();
            layer.map(|layer| (map, layer))
        }) {
            crate::life(format!("layer-shell: unmapped {}", layer.namespace()));
            map.unmap_layer(&layer);
        }
        if self.current_focus_surface().as_ref() == Some(&wl_surface) {
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, Option::<WlSurface>::None, SERIAL_COUNTER.next_serial());
            }
        }
        self.arrange_windows();
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_layer_popup(&popup);
    }
}

impl Smallvil {
    /// Arrange layer maps when a layer surface commits.
    ///
    pub fn handle_layer_commit(&mut self, surface: &WlSurface) {
        let output = self
            .space
            .outputs()
            .find(|o| {
                layer_map_for_output(o)
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned();
        let Some(output) = output else {
            return;
        };

        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|data| data.lock().unwrap().initial_configure_sent)
                .unwrap_or(true)
        });

        let mut map = layer_map_for_output(&output);
        let changed = map.arrange();
        if !initial_configure_sent {
            let layer = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned();
            if let Some(layer) = layer {
                crate::life(format!(
                    "layer-shell: initial configure for {}",
                    layer.namespace()
                ));
                layer.layer_surface().send_configure();
            }
        }
        drop(map);

        if changed || !initial_configure_sent {
            self.arrange_windows();
        }
    }

    /// Constrain an xdg popup parented to a layer-shell surface against the
    /// layer's output (quickshell menus attached to a bar).
    pub(crate) fn unconstrain_layer_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let output = self
            .space
            .outputs()
            .find(|o| {
                layer_map_for_output(o)
                    .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned()
            .or_else(|| self.output_under(Some(self.cursor_position)));
        let Some(output) = output else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return;
        };
        let layer = layer_map_for_output(&output)
            .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
            .cloned();
        let Some(layer) = layer else {
            return;
        };
        let Some(layer_geo) = layer_map_for_output(&output).layer_geometry(&layer) else {
            return;
        };

        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= output_geo.loc + layer_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

/// wlroots-style leniency for layer-surface commits, registered as a
/// pre-commit hook on every new surface (see `CompositorHandler::new_surface`).
pub(crate) fn layer_leniency_hook(
    _state: &mut Smallvil,
    _dh: &DisplayHandle,
    surface: &WlSurface,
) {
    if get_role(surface) != Some(LAYER_SURFACE_ROLE) {
        return;
    }
    with_states(surface, |states| {
        let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
        let pending = guard.pending();
        if pending.size.w == 0 && !pending.anchor.anchored_horizontally() {
            crate::life("layer-shell: clamped phantom zero width on post-unmap commit");
            pending.size.w = 1;
        }
        if pending.size.h == 0 && !pending.anchor.anchored_vertically() {
            crate::life("layer-shell: clamped phantom zero height on post-unmap commit");
            pending.size.h = 1;
        }
    });
}
