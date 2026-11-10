mod cli;

fn main() -> anyhow::Result<()> {
    match cli::parse(std::env::args_os().skip(1)).map_err(anyhow::Error::msg)? {
        cli::Command::Open => graph_editor::ui::run(),
        cli::Command::Help => {
            println!("{}", cli::USAGE);
            Ok(())
        }
    }
}
