use gpui::*;

/// Keep the baseline on a physical pixel without rounding glyph advances.
/// This avoids vertical softness at fractional Windows display scaling.
pub struct CrispText(pub SharedString);

pub fn aligned_origin(
    origin: Point<Pixels>,
    ascent: Pixels,
    descent: Pixels,
    height: Pixels,
    scale: f32,
) -> Point<Pixels> {
    let offset = (height - ascent - descent) / 2. + ascent;
    point(
        px((f32::from(origin.x) * scale).round() / scale),
        px((f32::from(origin.y + offset) * scale).round() / scale) - offset,
    )
}

impl IntoElement for CrispText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for CrispText {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, TextLayout) {
        self.0.request_layout(None, None, window, cx)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        mut bounds: Bounds<Pixels>,
        layout: &mut TextLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = window.text_style();
        let line = window.text_system().shape_line(
            self.0.clone(),
            style.font_size.to_pixels(window.rem_size()),
            &[style.to_run(self.0.len())],
            None,
        );
        bounds.origin = aligned_origin(
            bounds.origin,
            line.ascent,
            line.descent,
            window.line_height(),
            window.scale_factor(),
        );
        self.0.prepaint(None, None, bounds, layout, window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut TextLayout,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0
            .paint(None, None, bounds, layout, &mut (), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::aligned_origin;
    use gpui::{point, px};
    #[test]
    fn snaps_baseline_at_fractional_dpi_without_changing_text_size() {
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            let aligned =
                aligned_origin(point(px(3.2), px(7.3)), px(12.4), px(3.1), px(20.), scale);
            let baseline = (f32::from(aligned.y) + (20. - 12.4 - 3.1) / 2. + 12.4) * scale;
            assert!((baseline - baseline.round()).abs() < 0.0001);
        }
    }
}
