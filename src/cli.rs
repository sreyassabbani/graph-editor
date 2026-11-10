use std::ffi::{OsStr, OsString};

pub const USAGE: &str = "Usage: graph-editor [--help]";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Help,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, &'static str> {
    let mut args = args.into_iter();
    let command = match args.next() {
        None => Command::Open,
        Some(flag) if flag == OsStr::new("--help") || flag == OsStr::new("-h") => Command::Help,
        _ => return Err(USAGE),
    };
    if args.next().is_some() {
        return Err(USAGE);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_strings(args: &[&str]) -> Result<Command, &'static str> {
        parse(args.iter().map(OsString::from))
    }
    #[test]
    fn accepts_scratch_or_help_and_rejects_file_commands() {
        assert_eq!(parse_strings(&[]), Ok(Command::Open));
        assert_eq!(parse_strings(&["--help"]), Ok(Command::Help));
        for args in [
            &["graph.json"][..],
            &["--export", "graph.json"],
            &["--scratch"],
            &["--help", "extra"],
        ] {
            assert!(parse_strings(args).is_err());
        }
    }
}
