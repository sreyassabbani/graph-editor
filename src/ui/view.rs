use super::*;

#[derive(Clone, Copy)]
pub(super) enum ToolbarAction {
    Undo,
    Redo,
    Fit,
    Export,
    StyleAll,
    Copy,
    Label,
    Style,
    Delete,
    Zoom(f64),
}
const BUTTONS: &[(&str, ToolbarAction)] = &[
    (" Copy ", ToolbarAction::Copy),
    (" Save .typ ", ToolbarAction::Export),
    (" Undo ", ToolbarAction::Undo),
    (" Redo ", ToolbarAction::Redo),
    (" Fit ", ToolbarAction::Fit),
    (" - ", ToolbarAction::Zoom(1. / 1.2)),
    (" + ", ToolbarAction::Zoom(1.2)),
];
const SELECTED_BUTTONS: &[(&str, ToolbarAction)] = &[
    (" Label ", ToolbarAction::Label),
    (" Style ", ToolbarAction::Style),
    (" Delete ", ToolbarAction::Delete),
    (" Style all ", ToolbarAction::StyleAll),
];
fn menu(selected: bool) -> impl Iterator<Item = &'static (&'static str, ToolbarAction)> {
    BUTTONS
        .iter()
        .chain(if selected { SELECTED_BUTTONS } else { &[] }.iter())
}
pub(super) fn toolbar_action(col: u16, row: u16, selected: bool) -> Option<ToolbarAction> {
    if row != 0 {
        return None;
    }
    let mut start = 0;
    for &(text, action) in menu(selected) {
        let end = start + text.len();
        if (col as usize) < end {
            return Some(action);
        }
        start = end;
    }
    None
}

impl App {
    pub(super) fn scene(&self) -> canvas::Scene {
        canvas::Scene {
            state: CanvasState {
                graph: self.graph.clone(),
                camera: self.camera,
                selected: self.selected,
                edge_start: self.edge_start,
                // Kitty presents hover separately; fallback protocols bake it in.
                hover: if self.kitty.is_some() {
                    None
                } else {
                    self.hover
                },
                size: self.pixel_size(),
            },
            logical_size: self.size(),
        }
    }

    pub(super) fn draw(&mut self, f: &mut ratatui::Frame) {
        let full = f.area();
        if full.height < 2 || full.width == 0 {
            self.area = Rect::default();
            self.canvas_state = None;
            let _ = self.renderer.take();
            if let Some(canvas) = &mut self.kitty {
                canvas.hide();
            }
            return;
        }
        let header = if let Some(prompt) = &self.path_prompt {
            let suffix = if prompt.overwrite {
                "  Overwrite? [Enter/Esc]"
            } else {
                ""
            };
            let visible: String = prompt
                .input
                .chars()
                .rev()
                .take(full.width.saturating_sub(12 + suffix.len() as u16) as usize)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Line::from(format!("Save .typ: {visible}{suffix}"))
        } else if let Some(label) = &self.editing {
            Line::from(format!("Label: {label}"))
        } else {
            Line::from(
                menu(self.selected.is_some())
                    .map(|&(text, _)| Span::styled(text, Style::default().fg(Color::Blue)))
                    .collect::<Vec<_>>(),
            )
        };
        f.render_widget(Paragraph::new(header), Rect::new(0, 0, full.width, 1));
        self.area = Rect::new(0, 1, full.width, full.height - 1);
        let (w, h) = self.size();
        if self.fit_on_open {
            self.camera.fit(&self.graph, w, h);
            self.fit_on_open = false;
        }
        if self.scene_dirty
            || self
                .canvas_state
                .as_ref()
                .is_none_or(|s| s.size != self.pixel_size())
        {
            let scene = self.scene();
            if self.canvas_state.as_ref() != Some(&scene.state) {
                self.canvas_state = Some(scene.state.clone());
                self.renderer.submit(scene, self.area, self.kitty.is_some());
            }
            self.scene_dirty = false;
        }
        // A pre-resize image must not be placed over the new viewport.
        if let Some((_, result)) = self.renderer.take().filter(|(area, _)| *area == self.area) {
            match result {
                Ok(renderer::Frame::Kitty(encoded)) => {
                    if let Some(canvas) = &mut self.kitty {
                        canvas.stage_encoded(encoded);
                    }
                }
                Ok(renderer::Frame::Image(image)) => {
                    self.protocol = Some(self.picker.new_resize_protocol(image));
                }
                Err(error) => {
                    self.status = format!("Render failed: {error:#}");
                }
            }
        }
        let hover = self.hover_bounds();
        let physical = self.picker.font_size();
        let scale = (
            physical.0 as f64 / self.cell.0,
            physical.1 as f64 / self.cell.1,
        );
        if let Some(canvas) = &mut self.kitty {
            if let Err(error) = canvas.stage_hover(hover, self.area, physical, |w, h| {
                canvas::render_hover(w, h, scale)
            }) {
                self.status = format!("Hover failed: {error:#}");
            }
        }
        if let Some(protocol) = &mut self.protocol {
            f.render_stateful_widget(StatefulImage::default(), self.area, protocol);
        }
        // Only real operation results/errors appear; no reserved subtitle rows.
        if !self.status.is_empty() {
            f.render_widget(
                Paragraph::new(self.status.as_str()),
                Rect::new(0, full.height - 1, full.width, 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    #[test]
    fn canvas_fills_pane_below_single_row_on_open_resize_and_selection() {
        let mut picker = Picker::from_fontsize((19, 42));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(Graph::default(), picker);
        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.area, Rect::new(0, 1, 100, 59));
        assert!(app.status.is_empty());
        app.selected = Some(app.graph.add_vertex(0., 0.));
        app.scene_dirty = true;
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.area, Rect::new(0, 1, 100, 59));
        terminal.backend_mut().resize(80, 30);
        terminal.autoresize().unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.area, Rect::new(0, 1, 80, 29));
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.renderer.busy() || app.renderer.ready() {
            assert!(Instant::now() < deadline, "render worker did not finish");
            terminal.draw(|f| app.draw(f)).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut sequence = Vec::new();
        app.kitty.as_mut().unwrap().flush(&mut sequence).unwrap();
        assert!(String::from_utf8(sequence).unwrap().contains("c=80,r=29"));
    }
}
