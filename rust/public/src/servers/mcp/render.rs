use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;
use arrow::util::display::{ArrayFormatter, FormatOptions};
use futures::{Stream, StreamExt};

/// Caps on a rendered result.
#[derive(Debug, Clone, Copy)]
pub struct RenderLimits {
    pub max_rows: usize,
    pub max_bytes: usize,
}

#[derive(Debug)]
pub struct Rendered {
    pub text: String,
    pub rows: usize,
    pub truncated: bool,
}

fn escape_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace("\r\n", "\\n")
        .replace(['\n', '\r'], "\\n")
}

fn header(schema: &SchemaRef) -> String {
    let names: Vec<String> = schema
        .fields()
        .iter()
        .map(|f| escape_cell(f.name()))
        .collect();
    let rule = vec!["---"; names.len()].join(" | ");
    format!("| {} |\n| {rule} |\n", names.join(" | "))
}

fn truncation_footer(rows: usize) -> String {
    format!("\ntruncated at {rows} rows, add filters or aggregate\n")
}

fn cut_at_char_boundary(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Renders the stream as a markdown table. The stream is dropped as soon as the result is known
/// to exceed a cap, which cancels the query on the server.
pub async fn render_stream<S, E>(mut stream: S, limits: RenderLimits) -> Result<Rendered, E>
where
    S: Stream<Item = Result<RecordBatch, E>> + Unpin,
{
    let options = FormatOptions::default().with_null("NULL");
    let mut text = String::new();
    let mut rows = 0usize;
    let mut truncated = false;
    let mut header_written = false;
    // The footer is part of the output, so the body budget leaves room for it. `rows` never
    // exceeds `max_rows`, which bounds the footer's digit count.
    let body_cap = limits
        .max_bytes
        .saturating_sub(truncation_footer(limits.max_rows).len());

    'batches: while let Some(batch) = stream.next().await {
        let batch = batch?;
        if !header_written {
            let header = header(&batch.schema());
            header_written = true;
            if header.len() > body_cap {
                text.push_str(cut_at_char_boundary(&header, body_cap));
                truncated = true;
                break 'batches;
            }
            text.push_str(&header);
        }
        if batch.num_rows() == 0 {
            continue;
        }
        let formatters: Vec<_> = batch
            .columns()
            .iter()
            .map(|c| ArrayFormatter::try_new(c.as_ref(), &options))
            .collect();
        for row in 0..batch.num_rows() {
            if rows >= limits.max_rows {
                truncated = true;
                break 'batches;
            }
            let cells: Vec<String> = formatters
                .iter()
                .map(|f| match f {
                    Ok(f) => escape_cell(&f.value(row).to_string()),
                    Err(e) => format!("<{e}>"),
                })
                .collect();
            let mut line = format!("| {} |\n", cells.join(" | "));
            if text.len() + line.len() > body_cap {
                if rows == 0 {
                    // A first row that alone exceeds the cap is cut rather than dropped, so the
                    // caller sees something of the result.
                    let room = body_cap.saturating_sub(text.len() + "…\n".len());
                    line = format!("{}…\n", cut_at_char_boundary(&line, room));
                    text.push_str(&line);
                    rows += 1;
                }
                truncated = true;
                break 'batches;
            }
            text.push_str(&line);
            rows += 1;
        }
    }

    if !header_written || rows == 0 && !truncated {
        return Ok(Rendered {
            text: "no data".to_string(),
            rows: 0,
            truncated: false,
        });
    }
    if truncated {
        text.push_str(&truncation_footer(rows));
        // Only reachable when max_bytes is too small for even the header and footer.
        text.truncate(cut_at_char_boundary(&text, limits.max_bytes).len());
    }
    Ok(Rendered {
        text,
        rows,
        truncated,
    })
}

/// Cuts free text to `max_bytes` and notes the cut.
pub fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    format!(
        "{}\n... truncated at {max_bytes} bytes",
        cut_at_char_boundary(text, max_bytes)
    )
}
