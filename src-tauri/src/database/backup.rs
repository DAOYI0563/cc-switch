use rusqlite::types::ValueRef;
use rusqlite::Connection;

use crate::database::{lock_conn, Database};
use crate::error::AppError;

impl Database {
    /// Creates an in-memory SQL snapshot used only inside an encrypted,
    /// short-lived DPAPI rollback point.
    pub fn export_sql_string(&self) -> Result<String, AppError> {
        let conn = lock_conn!(self.conn);
        dump_sql(&conn)
    }
}

fn dump_sql(conn: &Connection) -> Result<String, AppError> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(database_error)?;
    let mut output =
        format!("PRAGMA foreign_keys=OFF;\nPRAGMA user_version={version};\nBEGIN TRANSACTION;\n");
    let mut statement = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE sql IS NOT NULL AND type IN ('table', 'index', 'trigger')
             ORDER BY type = 'table' DESC, name",
        )
        .map_err(database_error)?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut tables = Vec::new();
    let mut deferred = Vec::new();
    for (kind, name, sql) in objects {
        if name.starts_with("sqlite_") {
            continue;
        }
        if kind == "table" {
            tables.push(name);
            output.push_str(&sql);
            output.push_str(";\n");
        } else {
            deferred.push(sql);
        }
    }
    for table in tables {
        dump_table(conn, &table, &mut output)?;
    }
    for sql in deferred {
        output.push_str(&sql);
        output.push_str(";\n");
    }
    output.push_str("COMMIT;\nPRAGMA foreign_keys=ON;\n");
    Ok(output)
}

fn dump_table(conn: &Connection, table: &str, output: &mut String) -> Result<(), AppError> {
    let quoted_table = quote_identifier(table);
    let mut columns_statement = conn
        .prepare(&format!("PRAGMA table_info({quoted_table})"))
        .map_err(database_error)?;
    let columns = columns_statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    if columns.is_empty() {
        return Ok(());
    }
    let quoted_columns = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = conn
        .prepare(&format!("SELECT {quoted_columns} FROM {quoted_table}"))
        .map_err(database_error)?;
    let mut rows = statement.query([]).map_err(database_error)?;
    while let Some(row) = rows.next().map_err(database_error)? {
        let values = (0..columns.len())
            .map(|index| {
                row.get_ref(index)
                    .map_err(database_error)
                    .and_then(sql_value)
            })
            .collect::<Result<Vec<_>, _>>()?;
        output.push_str(&format!(
            "INSERT INTO {quoted_table} ({quoted_columns}) VALUES ({});\n",
            values.join(", ")
        ));
    }
    Ok(())
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn sql_value(value: ValueRef<'_>) -> Result<String, AppError> {
    match value {
        ValueRef::Null => Ok("NULL".to_string()),
        ValueRef::Integer(value) => Ok(value.to_string()),
        ValueRef::Real(value) if value.is_finite() => Ok(value.to_string()),
        ValueRef::Real(_) => Err(AppError::Database(
            "数据库回滚快照包含非有限浮点数".to_string(),
        )),
        ValueRef::Text(value) => Ok(format!(
            "'{}'",
            String::from_utf8_lossy(value).replace('\'', "''")
        )),
        ValueRef::Blob(value) => Ok(format!("X'{}'", hex(value))),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}
