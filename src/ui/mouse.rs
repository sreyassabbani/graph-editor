//! Terminal capabilities probed once at startup, and pixel-precise mouse
//! positions. Cell-sized positions make panning and dragging step 8 by 16
//! logical pixels at a time.
use super::*;
use base64::Engine as _;
use std::io::Write;

pub(super) struct Capabilities {
    /// SGR-Pixels mouse reports (mode 1016).
    pub pixel_mouse: bool,
    /// Kitty graphics from POSIX shared memory (t=s).
    pub shared_memory: bool,
}

/// Ask whether the terminal knows SGR-Pixels (DECRQM 1016) and, for Kitty
/// graphics, whether it reads shared memory (a 1x1 query image), followed by
/// a device attributes request every terminal answers, so unsupported
/// queries never wait out the timeout. Skipped inside tmux, which forwards
/// neither. Must run in raw mode before crossterm starts reading input.
pub(super) fn probe(kitty: bool) -> Capabilities {
    let none = Capabilities {
        pixel_mouse: false,
        shared_memory: false,
    };
    if graphics::is_tmux() {
        return none;
    }
    let object = kitty.then(graphics::shm::probe_object).flatten();
    let mut query = String::from("\x1b[?1016$p");
    if let Some(name) = &object {
        query.push_str(&format!(
            "\x1b_Gi=31,s=1,v=1,a=q,t=s,f=32;{}\x1b\\",
            base64::engine::general_purpose::STANDARD.encode(name.to_bytes())
        ));
    }
    query.push_str("\x1b[c");
    let reply = exchange(query.as_bytes());
    if let Some(name) = &object {
        graphics::shm::unlink(name);
    }
    let Some(reply) = reply else {
        return none;
    };
    Capabilities {
        pixel_mouse: reports_pixels(&reply),
        shared_memory: object.is_some() && String::from_utf8_lossy(&reply).contains("\x1b_Gi=31;OK"),
    }
}

fn exchange(query: &[u8]) -> Option<Vec<u8>> {
    let mut out = stdout();
    if out.write_all(query).and_then(|()| out.flush()).is_err() {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut reply = Vec::new();
    while !finished(&reply) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut fd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd; read() writes at most buf.len() bytes.
        if unsafe { libc::poll(&mut fd, 1, left.as_millis() as libc::c_int) } <= 0 {
            break;
        }
        let mut buf = [0u8; 256];
        let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        reply.extend_from_slice(&buf[..n as usize]);
    }
    Some(reply)
}

/// The device attributes reply (`CSI ? ... c`) arrives last.
fn finished(reply: &[u8]) -> bool {
    reply.windows(3).enumerate().any(|(i, w)| {
        w == b"\x1b[?"
            && reply[i + 3..]
                .iter()
                .find(|b| !(b.is_ascii_digit() || **b == b';'))
                == Some(&b'c')
    })
}

/// `CSI ? 1016 ; Ps $ y` with Ps 1 (set) or 2 (reset) means supported.
fn reports_pixels(reply: &[u8]) -> bool {
    let reply = String::from_utf8_lossy(reply);
    ["\x1b[?1016;1$y", "\x1b[?1016;2$y"]
        .iter()
        .any(|r| reply.contains(r))
}

impl App {
    /// Toolbar cell and canvas-local logical position for a mouse event.
    pub(super) fn pointer(&self, m: &MouseEvent) -> (u16, u16, Option<(f64, f64)>) {
        if !self.pixel_mouse {
            return (m.column, m.row, self.local(m.column, m.row));
        }
        let physical = self.picker.font_size();
        let (px, py) = (f64::from(m.column) + 0.5, f64::from(m.row) + 0.5);
        let col = (px / f64::from(physical.0)) as u16;
        let row = (py / f64::from(physical.1)) as u16;
        let p = self.local(col, row).map(|_| {
            (
                px * self.cell.0 / f64::from(physical.0) - f64::from(self.area.x) * self.cell.0,
                py * self.cell.1 / f64::from(physical.1) - f64::from(self.area.y) * self.cell.1,
            )
        });
        (col, row, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mode_report_parsing() {
        assert!(reports_pixels(b"\x1b[?1016;2$y\x1b[?62;22c"));
        assert!(!reports_pixels(b"\x1b[?1016;0$y\x1b[?62;22c"));
        assert!(!reports_pixels(b"\x1b[?62;22c"));
        assert!(finished(b"\x1b[?1016;2$y\x1b[?62;22c"));
        assert!(!finished(b"\x1b[?1016;2$y"));
        assert!(!finished(b"\x1b[?62;2"));
    }
    #[test]
    fn pixel_positions_are_fractional_cells() {
        let mut app = crate::ui::test_support::app();
        app.pixel_mouse = true;
        // 8x16 physical cells, canvas starting on row 2.
        let event = |column, row| MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let (col, row, p) = app.pointer(&event(83, 40));
        assert_eq!((col, row), (10, 2));
        let p = p.unwrap();
        assert!((p.0 - 83.5).abs() < 1e-9 && (p.1 - 8.5).abs() < 1e-9);
        assert_eq!(app.pointer(&event(83, 3)).2, None);
        assert_eq!(app.pointer(&event(83, 3)).1, 0);
    }
}
