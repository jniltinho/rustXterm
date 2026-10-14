//! Session output logging: raw bytes or plain text without escape sequences.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use alacritty_terminal::vte::{Parser, Perform};

/// How output is written to the log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Bytes exactly as received, escape sequences included.
    Raw,
    /// Printable text, tabs and newlines only.
    Plain,
}

pub(crate) struct Log {
    out: BufWriter<File>,
    plain: Option<Parser>,
    text: PlainText,
}

impl Log {
    /// Open `path` for appending.
    pub(crate) fn open(path: &Path, format: LogFormat) -> io::Result<Self> {
        let file = File::options().create(true).append(true).open(path)?;
        Ok(Self {
            out: BufWriter::new(file),
            plain: (format == LogFormat::Plain).then(Parser::new),
            text: PlainText::default(),
        })
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &mut self.plain {
            None => self.out.write_all(bytes)?,
            Some(parser) => {
                parser.advance(&mut self.text, bytes);
                self.out.write_all(&self.text.0)?;
                self.text.0.clear();
            }
        }
        self.out.flush()
    }
}

#[derive(Default)]
struct PlainText(Vec<u8>);

impl Perform for PlainText {
    fn print(&mut self, c: char) {
        let mut buf = [0; 4];
        self.0.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }

    fn execute(&mut self, byte: u8) {
        if matches!(byte, b'\n' | b'\t') {
            self.0.push(byte);
        }
    }
}
