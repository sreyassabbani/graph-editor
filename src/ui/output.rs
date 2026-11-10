use super::clipboard::copy_to_clipboard;
use super::*;

impl App {
    pub(super) fn export(&mut self) {
        self.path_prompt = Some(PathPrompt {
            input: self
                .export_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            overwrite: false,
        });
        self.status.clear();
    }
    pub(super) fn copy(&mut self) {
        self.status =
            match export::export_typst(&self.graph).and_then(|code| copy_to_clipboard(&code)) {
                Ok(()) => {
                    self.delivered = Some(self.graph.clone());
                    "Copied".into()
                }
                Err(e) => format!("Copy failed: {e:#}"),
            };
    }
    pub(super) fn path_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        match code {
            KeyCode::Esc => {
                self.path_prompt = None;
                self.status.clear();
            }
            KeyCode::Enter => self.submit_path(),
            _ => {
                if let Some(prompt) = &mut self.path_prompt {
                    match code {
                        KeyCode::Backspace => {
                            prompt.input.pop();
                        }
                        KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => {
                            prompt.input.clear()
                        }
                        KeyCode::Char(c)
                            if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            prompt.input.push(c)
                        }
                        _ => {}
                    }
                    prompt.overwrite = false;
                }
            }
        }
    }
    pub(super) fn submit_path(&mut self) {
        let Some(prompt) = &mut self.path_prompt else {
            return;
        };
        if prompt.input.trim().is_empty() {
            self.status = "Path required".into();
            return;
        }
        let mut path = if let Some(rest) = prompt.input.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(rest)
        } else {
            PathBuf::from(&prompt.input)
        };
        if path.extension().is_none() {
            path.set_extension("typ");
        }
        if path.exists() && !prompt.overwrite {
            prompt.overwrite = true;
            self.status = "File exists. Enter overwrites; Esc cancels.".into();
            return;
        }
        match export::write_typst(&path, &self.graph) {
            Ok(()) => {
                self.export_path = Some(path.clone());
                self.delivered = Some(self.graph.clone());
                self.status = format!("Saved {}", path.display());
                self.path_prompt = None;
            }
            Err(e) => {
                self.status = format!("Save failed: {e:#}");
                prompt.overwrite = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::app;
    #[test]
    fn scratch_export_prompts_and_protects_existing_files() {
        let mut app = app();
        let file = std::env::temp_dir().join(format!("graph-dialog-{}.typ", std::process::id()));
        std::fs::write(&file, "original").unwrap();
        app.export();
        app.path_prompt.as_mut().unwrap().input = file.display().to_string();
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
        assert!(app.path_prompt.as_ref().unwrap().overwrite);
        app.key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.path_prompt.is_none());
        app.export();
        app.path_prompt.as_mut().unwrap().input = file.display().to_string();
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(std::fs::read_to_string(&file)
            .unwrap()
            .contains("#context diagram"));
        assert!(app.key(KeyCode::Char('q'), KeyModifiers::NONE));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn failed_export_keeps_the_path_prompt_open() {
        let mut app = app();
        app.export();
        app.path_prompt.as_mut().unwrap().input = "/no-such-graph-directory/drawing.typ".into();
        app.submit_path();
        assert!(app.path_prompt.is_some());
        assert!(app.status.starts_with("Save failed"));
    }
}
