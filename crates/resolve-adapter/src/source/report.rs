use crate::Result;
use serde::Serialize;
use std::io::{self, Write};

pub(super) fn write(writer: &mut impl Write, report: &impl Serialize) -> Result<()> {
    let mut out = ReportWriter {
        writer,
        remaining: graph::MAX_GRAPH_BYTES,
    };
    serde_json::to_writer_pretty(&mut out, report)?;
    out.write_all(b"\n")?;
    Ok(())
}

struct ReportWriter<W> {
    writer: W,
    remaining: usize,
}

impl<W: Write> Write for ReportWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other("Resolve source report exceeds 64 MiB"));
        }
        let count = self.writer.write(bytes)?;
        self.remaining -= count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}
