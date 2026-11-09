//! A CSV or TSV file as a table, parsed once when the text arrives.

/// A preview is a glance, not a spreadsheet.
pub const MAX_CSV_ROWS: usize = 1_000;
pub const MAX_CSV_COLUMNS: usize = 100;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CsvTable {
    pub rows: Vec<Vec<String>>,
    pub width: usize,
    pub truncated: bool,
}

#[must_use]
pub fn delimiter(path: &str) -> char {
    if path.to_ascii_lowercase().ends_with(".tsv") {
        '\t'
    } else {
        ','
    }
}

/// RFC 4180 fields: `""` inside quotes is one quote, and a quoted field may
/// hold the delimiter or a newline. Stops at the caps instead of building
/// rows it would not show.
#[must_use]
pub fn parse(text: &str, delimiter: char) -> CsvTable {
    parse_capped(text, delimiter, MAX_CSV_ROWS, MAX_CSV_COLUMNS)
}

fn parse_capped(text: &str, delimiter: char, max_rows: usize, max_columns: usize) -> CsvTable {
    let mut table = CsvTable::default();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.strip_prefix('\u{feff}').unwrap_or(text).chars();
    let mut pending: Option<char> = None;
    let end_field = |row: &mut Vec<String>, field: &mut String, table: &mut CsvTable| {
        if row.len() < max_columns {
            row.push(std::mem::take(field));
        } else {
            field.clear();
            table.truncated = true;
        }
    };
    while let Some(ch) = pending.take().or_else(|| chars.next()) {
        if quoted {
            if ch == '"' {
                match chars.next() {
                    Some('"') => field.push('"'),
                    next => {
                        quoted = false;
                        pending = next;
                    }
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        if ch == '"' && field.is_empty() {
            quoted = true;
        } else if ch == delimiter {
            end_field(&mut row, &mut field, &mut table);
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' {
                match chars.next() {
                    Some('\n') | None => {}
                    next => pending = next,
                }
            }
            end_field(&mut row, &mut field, &mut table);
            table.width = table.width.max(row.len());
            table.rows.push(std::mem::take(&mut row));
            if table.rows.len() == max_rows {
                table.truncated |= pending.is_some() || chars.next().is_some();
                return table;
            }
        } else {
            field.push(ch);
        }
    }
    if !field.is_empty() || !row.is_empty() || quoted {
        end_field(&mut row, &mut field, &mut table);
        table.width = table.width.max(row.len());
        table.rows.push(row);
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(table: &CsvTable) -> Vec<Vec<&str>> {
        table
            .rows
            .iter()
            .map(|row| row.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn quoted_fields_hold_delimiters_quotes_and_newlines() {
        let table = parse("\u{feff}a,b\r\n\"x, y\",\"say \"\"hi\"\"\nthere\"\n", ',');
        assert_eq!(
            cells(&table),
            vec![vec!["a", "b"], vec!["x, y", "say \"hi\"\nthere"]]
        );
        assert_eq!(table.width, 2);
        assert!(!table.truncated);
    }

    #[test]
    fn a_tab_separates_a_tsv_and_a_last_line_needs_no_newline() {
        assert_eq!(delimiter("data/OUT.TSV"), '\t');
        assert_eq!(delimiter("a.csv"), ',');
        assert_eq!(
            cells(&parse("a\tb\n1\t2", '\t')),
            vec![["a", "b"], ["1", "2"]]
        );
    }

    #[test]
    fn rows_and_columns_stop_at_their_caps() {
        let table = parse_capped("1,2,3\n4,5,6\n7,8,9\n", ',', 2, 2);
        assert_eq!(cells(&table), vec![["1", "2"], ["4", "5"]]);
        assert!(table.truncated);
        let exact = parse_capped("1\n2\n", ',', 2, 2);
        assert!(!exact.truncated, "nothing was left out");
    }
}
