#![allow(dead_code)]

use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for CsvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CSV parse error at line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl Error for CsvError {}

pub fn parse_csv(input: &str) -> Result<Vec<Vec<String>>, CsvError> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut chars = input.chars().peekable();
    let mut in_quotes = false;
    let mut at_field_start = true;
    let mut line = 1usize;
    let mut col = 1usize;
    let mut row_has_data = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quotes {
                    if matches!(chars.peek(), Some('"')) {
                        chars.next();
                        field.push('"');
                        col += 1;
                    } else {
                        in_quotes = false;
                    }
                } else if at_field_start {
                    in_quotes = true;
                    row_has_data = true;
                } else {
                    return Err(CsvError {
                        line,
                        column: col,
                        message: "unexpected quote in unquoted field".to_string(),
                    });
                }
                at_field_start = false;
            }
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                at_field_start = true;
                row_has_data = true;
            }
            '\n' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_field_start = true;
                row_has_data = false;
                line += 1;
                col = 0;
            }
            '\r' if !in_quotes => {
                if matches!(chars.peek(), Some('\n')) {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_field_start = true;
                row_has_data = false;
                line += 1;
                col = 0;
            }
            c => {
                field.push(c);
                at_field_start = false;
                row_has_data = true;
                if c == '\n' {
                    line += 1;
                    col = 0;
                }
            }
        }
        col += 1;
    }

    if in_quotes {
        return Err(CsvError {
            line,
            column: col,
            message: "unterminated quoted field".to_string(),
        });
    }

    if !field.is_empty() || !row.is_empty() || row_has_data {
        row.push(field);
        rows.push(row);
    }

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::parse_csv;

    #[test]
    fn parses_basic_csv() {
        let out = parse_csv("a,b,c\n1,2,3").expect("basic csv should parse");
        assert_eq!(out, vec![vec!["a", "b", "c"], vec!["1", "2", "3"]]);
    }

    #[test]
    fn handles_empty_fields_and_trailing_comma() {
        let out = parse_csv("a,,c,\n,,").expect("csv with empty fields should parse");
        assert_eq!(out, vec![vec!["a", "", "c", ""], vec!["", "", ""]]);
    }

    #[test]
    fn handles_quoted_commas_and_newlines() {
        let out = parse_csv("name,notes\n\"Doe, Jane\",\"line1\nline2\"")
            .expect("csv with quoted commas/newlines should parse");
        assert_eq!(
            out,
            vec![vec!["name", "notes"], vec!["Doe, Jane", "line1\nline2"]]
        );
    }

    #[test]
    fn handles_escaped_quotes() {
        let out =
            parse_csv("\"He said \"\"hi\"\"\",ok").expect("csv with escaped quotes should parse");
        assert_eq!(out, vec![vec!["He said \"hi\"", "ok"]]);
    }

    #[test]
    fn handles_multiple_quoted_commas_across_rows() {
        let out = parse_csv(
            "id,name,city\n1,\"Smith, John\",\"New York, NY\"\n2,\"Doe, Jane\",\"Austin, TX\"",
        )
        .expect("csv with multiple quoted comma rows should parse");
        assert_eq!(
            out,
            vec![
                vec!["id", "name", "city"],
                vec!["1", "Smith, John", "New York, NY"],
                vec!["2", "Doe, Jane", "Austin, TX"]
            ]
        );
    }

    #[test]
    fn handles_escaped_quotes_with_commas() {
        let out = parse_csv("text\n\"She said \"\"go, now\"\" and left\"")
            .expect("csv with escaped quotes+commas should parse");
        assert_eq!(
            out,
            vec![vec!["text"], vec!["She said \"go, now\" and left"]]
        );
    }

    #[test]
    fn handles_crlf() {
        let out = parse_csv("a,b\r\n1,2\r\n").expect("csv with crlf should parse");
        assert_eq!(out, vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn errors_on_unterminated_quote() {
        let err = parse_csv("\"a,b").expect_err("unterminated quote should error");
        assert!(err.message.contains("unterminated quoted field"));
    }

    #[test]
    fn errors_on_unexpected_quote() {
        let err = parse_csv("ab\"cd,1").expect_err("unexpected quote should error");
        assert!(err.message.contains("unexpected quote"));
    }
}
