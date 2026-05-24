use anyhow::{Context, Result};
use std::io::{self, Write};

pub(crate) fn write_stdout(content: &str) -> Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    write_all_or_ignore_broken_pipe(&mut stdout, content)
}

pub(crate) fn write_stdout_line(content: &str) -> Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    write_all_or_ignore_broken_pipe(&mut stdout, content)?;
    write_all_or_ignore_broken_pipe(&mut stdout, "\n")
}

fn write_all_or_ignore_broken_pipe<W: Write>(writer: &mut W, content: &str) -> Result<()> {
    match writer.write_all(content.as_bytes()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err).context("failed to write to stdout"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_broken_pipe_is_not_an_error() {
        let mut writer = BrokenPipeWriter;
        write_all_or_ignore_broken_pipe(&mut writer, "[]").expect("broken pipe is ignored");
    }
}
