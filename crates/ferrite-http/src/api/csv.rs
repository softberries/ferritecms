//! A small CSV reader and writer for the redirect import and export (P29, RFC 4180).
//!
//! **Reading** ([`parse`]): fields are separated by `,` and records by CRLF or LF; a
//! field may be quoted with `"`, and then holds commas, line breaks and `""` (one
//! quote). A leading byte-order mark is skipped; empty lines are skipped; the last
//! record may end without a line break. Errors name the line: a quote inside an
//! unquoted field, text after a closing quote, a quoted field that never ends, a bare
//! CR (not followed by LF), a NUL character.
//!
//! **Writing** ([`write_row`]): a field is quoted when it holds `,`, `"`, CR or LF, or
//! starts or ends with a space; quotes are doubled; the row ends with CRLF. A field
//! that starts with `=`, `+`, `-`, `@`, a tab or CR gets a leading `'`, so a spreadsheet
//! opening the file does not run it as a formula (CSV injection). No valid redirect
//! source (`/…`), target (`/…`, `http…`) or code starts with one of them, so an export
//! imports back unchanged.

/// One record and the line it starts on (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvRecord {
    pub line: usize,
    pub fields: Vec<String>,
}

/// What is wrong with the input, and on which line (1-based).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {detail}")]
pub struct CsvError {
    pub line: usize,
    pub detail: String,
}

/// The records of `input`; see the module docs.
pub fn parse(input: &str) -> Result<Vec<CsvRecord>, CsvError> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut records = Vec::new();
    let mut chars = input.chars().peekable();
    let mut line = 1;
    let err = |line: usize, detail: &str| CsvError {
        line,
        detail: detail.to_owned(),
    };
    while chars.peek().is_some() {
        let start = line;
        let mut fields = Vec::new();
        let mut field = String::new();
        // `true` once the current field had a closing quote.
        let mut closed = false;
        let mut quoted = false;
        loop {
            let c = chars.next();
            match c {
                None => {
                    if quoted && !closed {
                        return Err(err(start, "a quoted field is not closed"));
                    }
                    fields.push(std::mem::take(&mut field));
                    break;
                }
                Some('\0') => return Err(err(line, "NUL character")),
                Some(c) if quoted && !closed => {
                    if c == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            field.push('"');
                        } else {
                            closed = true;
                        }
                    } else {
                        if c == '\n' {
                            line += 1;
                        }
                        field.push(c);
                    }
                }
                Some(',') => {
                    fields.push(std::mem::take(&mut field));
                    quoted = false;
                    closed = false;
                }
                Some('\r') => {
                    if chars.next() != Some('\n') {
                        return Err(err(line, "a CR must be followed by LF"));
                    }
                    line += 1;
                    fields.push(std::mem::take(&mut field));
                    break;
                }
                Some('\n') => {
                    line += 1;
                    fields.push(std::mem::take(&mut field));
                    break;
                }
                Some('"') if field.is_empty() && !quoted => quoted = true,
                Some(_) if closed => {
                    return Err(err(line, "text after a closing quote"));
                }
                Some('"') => {
                    return Err(err(line, "a quote inside an unquoted field"));
                }
                Some(c) => field.push(c),
            }
        }
        // An empty line is one empty unquoted field: skipped.
        let empty = fields.len() == 1 && fields[0].is_empty() && !quoted;
        if !empty {
            records.push(CsvRecord {
                line: start,
                fields,
            });
        }
    }
    Ok(records)
}

/// One CSV row with CRLF; see the module docs.
pub fn write_row(fields: &[&str]) -> String {
    let mut out = String::new();
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let guard = field.starts_with(['=', '+', '-', '@', '\t', '\r']);
        let value = if guard {
            format!("'{field}")
        } else {
            (*field).to_owned()
        };
        let quote = value.contains([',', '"', '\r', '\n'])
            || value.starts_with(' ')
            || value.ends_with(' ');
        if quote {
            out.push('"');
            out.push_str(&value.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(&value);
        }
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(input: &str) -> Vec<Vec<String>> {
        parse(input)
            .unwrap()
            .into_iter()
            .map(|r| r.fields)
            .collect()
    }

    #[test]
    fn reads_rfc_4180() {
        assert_eq!(fields("a,b,c"), [["a", "b", "c"]]);
        assert_eq!(fields("a,b\r\nc,d\r\n"), [["a", "b"], ["c", "d"]]);
        assert_eq!(fields("a,b\nc,d\n"), [["a", "b"], ["c", "d"]]);
        assert_eq!(fields("\u{feff}a,b"), [["a", "b"]]);
        assert_eq!(
            fields(r#""a,b","say ""hi""",c"#),
            [["a,b", "say \"hi\"", "c"]]
        );
        assert_eq!(fields("\"two\r\nlines\",x"), [["two\r\nlines", "x"]]);
        assert_eq!(fields("a,,\n"), [["a", "", ""]]);
        assert_eq!(fields("\"\""), [[""]]);
        assert_eq!(fields("a\n\n\r\nb"), [["a"], ["b"]]);
        assert!(fields("").is_empty());
    }

    #[test]
    fn records_know_their_start_line() {
        let r = parse("h\n\"x\ny\",1\nz,2\n").unwrap();
        let lines: Vec<usize> = r.iter().map(|r| r.line).collect();
        assert_eq!(lines, [1, 2, 4]);
    }

    #[test]
    fn errors_name_the_line() {
        let e = |input: &str| parse(input).unwrap_err();
        assert_eq!(e("ok\nab\"c").line, 2);
        assert!(e("ok\nab\"c").detail.contains("unquoted"));
        assert_eq!(e("ok\n\"a\"b").line, 2);
        assert_eq!(e("ok\nok\n\"open\nmore").line, 3);
        assert!(e("\"open").detail.contains("not closed"));
        assert_eq!(e("a\rb").line, 1);
        assert_eq!(e("a\n\0").line, 2);
    }

    #[test]
    fn writes_and_reads_back() {
        let rows: [&[&str]; 4] = [
            &["/a", "https://x.example/?q=1,2", "301"],
            &["/b", "say \"hi\"", "302"],
            &["/c", " padded ", "307"],
            &["/d", "two\nlines", "308"],
        ];
        let mut text = String::new();
        for r in rows {
            text.push_str(&write_row(r));
        }
        let back = fields(&text);
        for (r, b) in rows.iter().zip(&back) {
            assert_eq!(r.to_vec(), *b);
        }
        assert_eq!(write_row(&["a", "b"]), "a,b\r\n");
    }

    #[test]
    fn formula_cells_are_neutralised() {
        assert_eq!(write_row(&["=1+1"]), "'=1+1\r\n");
        assert_eq!(write_row(&["+x", "-y", "@z"]), "'+x,'-y,'@z\r\n");
        assert_eq!(write_row(&["=a,b"]), "\"'=a,b\"\r\n");
        assert_eq!(write_row(&["\tx"]), "'\tx\r\n");
        assert_eq!(write_row(&["/ok=1"]), "/ok=1\r\n");
    }
}
