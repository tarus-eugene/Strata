use gpui::*;

/// Keep the baseline on a physical pixel without rounding glyph advances.
/// This avoids vertical softness at fractional Windows display scaling.
pub struct CrispText(
    pub SharedString,
    pub Vec<(std::ops::Range<usize>, HighlightStyle)>,
    pub Option<std::rc::Rc<std::cell::Cell<bool>>>,
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

fn text_runs(
    text: &str,
    highlights: &[(std::ops::Range<usize>, HighlightStyle)],
    style: &TextStyle,
) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut offset = 0;
    for (range, highlight) in highlights {
        if offset < range.start {
            runs.push(style.to_run(range.start - offset));
        }
        runs.push(style.clone().highlight(*highlight).to_run(range.len()));
        offset = range.end;
    }
    if offset < text.len() {
        runs.push(style.to_run(text.len() - offset));
    }
    runs
}

fn measured_line_size(
    natural: Size<Pixels>,
    known: Size<Option<Pixels>>,
    available: Size<AvailableSpace>,
) -> Size<Pixels> {
    size(
        known.width.unwrap_or_else(|| match available.width {
            AvailableSpace::Definite(width) => natural.width.min(width),
            _ => natural.width,
        }),
        known.height.unwrap_or(natural.height),
    )
}

impl IntoElement for CrispText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for CrispText {
    type RequestLayoutState = (TextLayout, Option<StyledText>, Option<Vec<TextRun>>);
    type PrepaintState = Option<(ShapedLine, Point<Pixels>)>;

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
        let style = window.text_style();
        if style.white_space == WhiteSpace::Nowrap && style.text_overflow.is_some() {
            let runs = text_runs(&self.0, &self.1, &style);
            let line = window.text_system().shape_line(
                self.0.clone(),
                style.font_size.to_pixels(window.rem_size()),
                &runs,
                None,
            );
            let natural = size(line.width, window.line_height());
            // GPUI caches no-wrap truncation during flex measurement; truncate only at the final bounds.
            let id = window
                .request_measured_layout(Style::default(), move |known, available, _, _| {
                    measured_line_size(natural, known, available)
                });
            return (id, (TextLayout::default(), None, Some(runs)));
        }
        if self.1.is_empty() {
            let (id, layout) = self.0.request_layout(None, None, window, cx);
            return (id, (layout, None, None));
        }
        let mut styled = StyledText::new(self.0.clone()).with_highlights(self.1.clone());
        let (id, ()) = styled.request_layout(None, None, window, cx);
        (id, (TextLayout::default(), Some(styled), None))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        mut bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        if let Some(runs) = &layout.2 {
            let mut runs = runs.clone();
            let Some(TextOverflow::Truncate(suffix)) = style.text_overflow.clone() else {
                unreachable!()
            };
            let text = window
                .text_system()
                .line_wrapper(style.font(), style.font_size.to_pixels(window.rem_size()))
                .truncate_line(self.0.clone(), bounds.size.width, &suffix, &mut runs);
            let shortened = text != self.0;
            let line = window.text_system().shape_line(
                text,
                style.font_size.to_pixels(window.rem_size()),
                &runs,
                None,
            );
            if let Some(truncated) = &self.2 {
                truncated.set(shortened || line.width > bounds.size.width);
            }
            bounds.origin = aligned_origin(
                bounds.origin,
                line.ascent,
                line.descent,
                window.line_height(),
                window.scale_factor(),
            );
            bounds.origin.x += match style.text_align {
                TextAlign::Left => px(0.),
                TextAlign::Center => (bounds.size.width - line.width).max(px(0.)) / 2.,
                TextAlign::Right => (bounds.size.width - line.width).max(px(0.)),
            };
            return Some((line, bounds.origin));
        }
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
        None
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        painted_line: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((line, origin)) = painted_line {
            if let Err(error) = line
                .paint_background(*origin, window.line_height(), window, cx)
                .and_then(|_| line.paint(*origin, window.line_height(), window, cx))
            {
                eprintln!("Could not paint text: {error:#}");
            }
            return;
        }
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
    fn final_width_is_not_stuck_at_an_earlier_zero_width_measurement() {
        use gpui::{AvailableSpace, size};
        let natural = size(px(460.), px(20.));
        let known = size(None, None);
        let available = |width| {
            size(
                AvailableSpace::Definite(px(width)),
                AvailableSpace::MinContent,
            )
        };
        assert_eq!(
            super::measured_line_size(natural, known, available(0.)).width,
            px(0.)
        );
        assert_eq!(
            super::measured_line_size(natural, known, available(360.)).width,
            px(360.)
        );
        assert_eq!(
            super::measured_line_size(natural, known, available(600.)).width,
            px(460.)
        );
        assert_eq!(
            super::measured_line_size(natural, size(Some(px(280.)), None), available(0.)).width,
            px(280.)
        );
        let text = "Résumé download.iso";
        let highlights = vec![(
            0..8,
            gpui::HighlightStyle {
                font_weight: Some(gpui::FontWeight::BOLD),
                ..Default::default()
            },
        )];
        let runs = super::text_runs(text, &highlights, &gpui::TextStyle::default());
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert_eq!(runs[0].font.weight, gpui::FontWeight::BOLD);
    }
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
