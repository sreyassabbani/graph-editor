//! One render worker, with one replaceable pending scene. Input never waits for
//! SVG parsing, font shaping, rasterization, compression or frame transport.
//! Completed frames stay available during continuous gestures; pending work is
//! replaced by the latest scene, so slow rendering cannot build an input backlog.
use super::*;
use std::sync::{Arc, Condvar, Mutex};

pub(super) enum Frame {
    Kitty(graphics::KittyFrame),
    Image(image::DynamicImage),
}

struct Job {
    scene: canvas::Scene,
    area: Rect,
    /// Kitty transport; `None` for other graphics protocols.
    kitty: Option<graphics::Transport>,
}

#[derive(Default)]
struct Mailbox {
    next: Option<Job>,
    completed: Option<(Rect, Result<Frame>)>,
    working: bool,
    stopped: bool,
}

pub(super) struct Renderer {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}

impl Renderer {
    pub(super) fn new() -> Self {
        Self::start(render)
    }

    fn start(mut render: impl FnMut(&Job) -> Result<Frame> + Send + 'static) -> Self {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("graph-render".into())
            .spawn(move || {
                let (lock, wake) = &*worker;
                loop {
                    let job = {
                        let mut mailbox = lock.lock().unwrap();
                        while mailbox.next.is_none() && !mailbox.stopped {
                            mailbox = wake.wait(mailbox).unwrap();
                        }
                        if mailbox.stopped {
                            return;
                        }
                        mailbox.working = true;
                        mailbox.next.take().unwrap()
                    };
                    let result = render(&job);
                    let mut mailbox = lock.lock().unwrap();
                    mailbox.working = false;
                    if !mailbox.stopped {
                        mailbox.completed = Some((job.area, result));
                        wake.notify_all();
                    }
                }
            })
            .expect("starting graph render worker");
        Self { shared }
    }

    pub(super) fn submit(
        &self,
        scene: canvas::Scene,
        area: Rect,
        kitty: Option<graphics::Transport>,
    ) {
        let mut mailbox = self.shared.0.lock().unwrap();
        mailbox.next = Some(Job { scene, area, kitty });
        self.shared.1.notify_one();
    }
    pub(super) fn ready(&self) -> bool {
        self.shared.0.lock().unwrap().completed.is_some()
    }
    pub(super) fn busy(&self) -> bool {
        let mailbox = self.shared.0.lock().unwrap();
        // Include completed work: it may arrive between the event loop's ready
        // check and this check. Never block indefinitely waiting for input then.
        mailbox.working || mailbox.next.is_some() || mailbox.completed.is_some()
    }
    pub(super) fn take(&self) -> Option<(Rect, Result<Frame>)> {
        self.shared.0.lock().unwrap().completed.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn slow_render_does_not_block_submission_or_queue_obsolete_scenes() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let renderer = Renderer::start(move |job| {
            let id = job.scene.state.graph.next_id;
            started_tx.send(id).unwrap();
            release_rx.recv().unwrap();
            Ok(Frame::Kitty(graphics::KittyFrame {
                upload: id.to_string(),
                camera: job.scene.state.camera,
                margin: (0, 0),
            }))
        });
        let app = super::super::test_support::app();
        let submit = |id| {
            let mut scene = app.scene();
            scene.state.graph.next_id = id;
            renderer.submit(scene, app.area, Some(graphics::Transport::Inline));
        };
        submit(1);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        // Rendering is deliberately blocked; input can still replace requests.
        submit(2);
        submit(3);
        release_tx.send(()).unwrap();
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 3);
        // Continuous input must not starve presentation of completed frames.
        let (_, frame) = renderer.take().unwrap();
        assert!(matches!(frame.unwrap(), Frame::Kitty(f) if f.upload == "1"));
        assert!(renderer.busy());
        release_tx.send(()).unwrap();
        let mailbox = renderer.shared.0.lock().unwrap();
        let (mailbox, timeout) = renderer
            .shared
            .1
            .wait_timeout_while(mailbox, Duration::from_secs(2), |m| m.completed.is_none())
            .unwrap();
        assert!(!timeout.timed_out());
        drop(mailbox);
        assert!(
            renderer.busy(),
            "completed frames must prevent a blocking input wait"
        );
        let (_, frame) = renderer.take().unwrap();
        assert!(matches!(frame.unwrap(), Frame::Kitty(f) if f.upload == "3"));
        assert!(!renderer.busy());
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let mut mailbox = self.shared.0.lock().unwrap();
        mailbox.stopped = true;
        mailbox.next = None;
        self.shared.1.notify_one();
    }
}

fn render(job: &Job) -> Result<Frame> {
    let pix = job.scene.render()?;
    if let Some(transport) = job.kitty {
        Ok(Frame::Kitty(graphics::KittyFrame {
            upload: graphics::encode_frame(pix, transport, graphics::is_tmux())?,
            camera: job.scene.state.camera,
            margin: job.scene.margin,
        }))
    } else {
        let (width, height) = (pix.width(), pix.height());
        let rgba = graphics::unpremultiply(pix);
        let image =
            image::RgbaImage::from_raw(width, height, rgba).context("canvas pixel buffer")?;
        Ok(Frame::Image(image.into()))
    }
}
