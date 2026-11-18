use super::*;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

const FRAME_INTERVAL: Duration = Duration::from_millis(8);

// Only adjacent motion samples are replaceable. Never drop a press, release,
// scroll, or key: those are gesture boundaries and ordered edits.
fn replaces(previous: &Event, next: &Event) -> bool {
    match (previous, next) {
        (Event::Mouse(a), Event::Mouse(b)) => {
            a.modifiers == b.modifiers
                && a.kind == b.kind
                && matches!(a.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
        }
        (Event::Resize(..), Event::Resize(..)) => true,
        _ => false,
    }
}

impl App {
    pub(super) fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let mut last_frame = Instant::now() - FRAME_INTERVAL;
        loop {
            self.dirty |= self.renderer.ready();
            self.dirty |= self.expire_status();
            if self.dirty && last_frame.elapsed() >= FRAME_INTERVAL {
                // Measure start-to-start; expensive work must not add another
                // fixed frame interval after it finishes.
                last_frame = Instant::now();
                // Commit text, image upload and placement together, avoiding
                // partially painted frames on terminals supporting DEC 2026.
                execute!(stdout(), BeginSynchronizedUpdate)?;
                let result = terminal.draw(|f| self.draw(f));
                let image = self
                    .kitty
                    .as_mut()
                    .map(|canvas| canvas.flush(&mut stdout()))
                    .transpose();
                let end = execute!(stdout(), EndSynchronizedUpdate);
                result?;
                image?;
                end?;
                self.dirty = false;
            }
            let wait = if self.dirty {
                Some(FRAME_INTERVAL.saturating_sub(last_frame.elapsed()))
            } else if self.renderer.busy() {
                Some(FRAME_INTERVAL)
            } else {
                // Wake to clear an operation result.
                self.status_at
                    .map(|at| STATUS_TTL.saturating_sub(at.elapsed()))
            };
            if let Some(wait) = wait {
                if !event::poll(wait)? {
                    continue;
                }
            }
            let mut pending = event::read()?;
            // Bound each batch so sustained mouse input cannot starve rendering.
            for _ in 0..255 {
                if !event::poll(Duration::ZERO)? {
                    break;
                }
                let next = event::read()?;
                if !replaces(&pending, &next) && self.handle_event(pending) {
                    return Ok(());
                }
                pending = next;
            }
            if self.handle_event(pending) {
                return Ok(());
            }
        }
    }

    /// Clear an operation result after [`STATUS_TTL`]; true if it changed.
    /// A pending quit confirmation stays until answered.
    pub(super) fn expire_status(&mut self) -> bool {
        if self.quit_pending || !self.status_at.is_some_and(|at| at.elapsed() >= STATUS_TTL) {
            return false;
        }
        self.status.clear();
        self.status_at = None;
        true
    }

    fn handle_event(&mut self, event: Event) -> bool {
        // Pure pointer motion cannot change graph geometry or the camera.
        // Keep its hot path independent of graph size and snapshot allocation.
        let hover_only = matches!(&event, Event::Mouse(m) if m.kind == MouseEventKind::Moved);
        match event {
            Event::Key(k) if k.kind == KeyEventKind::Press => {
                if self.key(k.code, k.modifiers) {
                    return true;
                }
            }
            Event::Mouse(m) => {
                let previous = (self.hover, self.toolbar_hover);
                self.mouse(m);
                if m.kind == MouseEventKind::Moved && previous == (self.hover, self.toolbar_hover) {
                    return false;
                }
            }
            Event::Resize(..) => {
                self.hover = None;
            }
            _ => return false,
        }
        self.dirty = true;
        self.scene_dirty |= !hover_only || self.kitty.is_none();
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mouse(kind: MouseEventKind, column: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row: 12,
            modifiers: KeyModifiers::NONE,
        })
    }
    #[test]
    fn retina_geometry_uses_logical_units_but_image_keeps_native_pixels() {
        let app = App::new(Graph::default(), {
            let mut picker = Picker::from_fontsize((19, 42));
            picker.set_protocol_type(ProtocolType::Kitty);
            picker
        });
        assert!(app.cell.0 <= 8. && app.cell.1 <= 16.);
        let physical = 19 * 42;
        let logical = app.cell.0 as u32 * app.cell.1 as u32;
        assert!(logical * 6 < physical);
        let mut app = app;
        app.area = Rect::new(0, 1, 100, 60);
        assert_eq!(app.pixel_size(), (1900, 2520));
    }
    #[test]
    fn coalesces_motion_without_losing_gesture_boundaries() {
        let drag = MouseEventKind::Drag(MouseButton::Left);
        assert!(replaces(&mouse(drag, 40), &mouse(drag, 50)));
        assert!(!replaces(
            &mouse(MouseEventKind::Down(MouseButton::Left), 40),
            &mouse(drag, 50)
        ));
        assert!(!replaces(
            &mouse(drag, 40),
            &mouse(MouseEventKind::Up(MouseButton::Left), 50)
        ));
        assert!(!replaces(
            &mouse(MouseEventKind::Moved, 40),
            &mouse(drag, 50)
        ));
    }
    #[test]
    fn hover_inside_same_grid_cell_does_not_schedule_another_frame() {
        let mut app = super::super::test_support::app();
        app.handle_event(mouse(MouseEventKind::Moved, 40));
        assert!(app.dirty);
        app.dirty = false;
        app.handle_event(mouse(MouseEventKind::Moved, 41));
        assert!(!app.dirty);
        app.handle_event(mouse(MouseEventKind::Moved, 46));
        assert!(app.dirty);
    }

    #[test]
    fn kitty_hover_does_not_invalidate_graph_scene_but_edits_do() {
        let mut picker = Picker::from_fontsize((19, 42));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(Graph::default(), picker);
        app.area = Rect::new(0, 1, 100, 60);
        app.scene_dirty = false;
        app.handle_event(mouse(MouseEventKind::Moved, 40));
        assert!(app.dirty);
        assert!(!app.scene_dirty);
        app.handle_event(mouse(MouseEventKind::Down(MouseButton::Left), 40));
        assert!(app.scene_dirty);
    }
}
