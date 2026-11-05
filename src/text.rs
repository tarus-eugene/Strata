use gpui::*;

/// Keep the baseline on a physical pixel without rounding glyph advances.
/// This avoids vertical softness at fractional Windows display scaling.
pub struct CrispText(
    pub SharedString,
    pub Vec<(std::ops::Range<usize>, HighlightStyle)>,
);

pub fn search_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut folded = String::new();
    let mut origins = Vec::new();
    for (start, character) in text.char_indices() {
        for lower in character.to_lowercase() {
            folded.push(lower);
            origins.extend(std::iter::repeat_n(
                start..start + character.len_utf8(),
                lower.len_utf8(),
            ));
        }
    }
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (start, matched) in folded.match_indices(&query.to_lowercase()) {
        let range = origins[start].start..origins[start + matched.len() - 1].end;
        if let Some(previous) = ranges
            .last_mut()
            .filter(|previous| previous.end > range.start)
        {
            previous.end = range.end;
        } else {
            ranges.push(range);
        }
    }
    ranges
}

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
    type RequestLayoutState = (TextLayout, Option<StyledText>);
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        if self.1.is_empty() {
            let (id, layout) = self.0.request_layout(None, None, window, cx);
            return (id, (layout, None));
        }
        let mut styled = StyledText::new(self.0.clone()).with_highlights(self.1.clone());
        let (id, ()) = styled.request_layout(None, None, window, cx);
        (id, (TextLayout::default(), Some(styled)))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        mut bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
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
        if let Some(styled) = &mut layout.1 {
            styled.prepaint(None, None, bounds, &mut (), window, cx);
        } else {
            self.0
                .prepaint(None, None, bounds, &mut layout.0, window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(styled) = &mut layout.1 {
            styled.paint(None, None, bounds, &mut (), &mut (), window, cx);
        } else {
            self.0
                .paint(None, None, bounds, &mut layout.0, &mut (), window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::aligned_origin;
    use gpui::{point, px};
    #[test]
    fn search_highlights_use_original_unicode_byte_boundaries() {
        use super::search_ranges;
        assert_eq!(search_ranges("Movie movie", "MOVIE"), vec![0..5, 6..11]);
        assert_eq!(search_ranges("Résumé.iso", "RÉSUMÉ"), vec![0..8]);
        assert_eq!(search_ranges("İSO.iso", "iso"), vec![5..8]);
        assert_eq!(search_ranges("İSO", "i"), vec![0..2]);
        assert!(search_ranges("anything", "").is_empty());
        assert!(search_ranges("anything", "missing").is_empty());
    }
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
