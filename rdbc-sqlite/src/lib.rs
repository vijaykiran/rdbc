//! SQLite RDBC Driver
//!
//! This crate implements an RDBC Driver for the `rusqlite` crate.
//!
//! The RDBC (Rust DataBase Connectivity) API is loosely based on the ODBC and JDBC standards.
//!
//! ```rust
//! use rdbc::Value;
//! use rdbc_sqlite::SqliteDriver;
//! let driver = SqliteDriver::new();
//! let conn = driver.connect_in_memory().unwrap();
//! let mut conn = conn.borrow_mut();
//! let stmt = conn.prepare("CREATE TABLE test (a INT NOT NULL)").unwrap().borrow_mut().execute_update(&vec![]).unwrap();
//! let stmt = conn.prepare("INSERT INTO test (a) VALUES (?)").unwrap().borrow_mut().execute_update(&vec![rdbc::Value::Int32(123)]).unwrap();
//! let stmt = conn.prepare("SELECT a FROM test").unwrap();
//! let mut stmt = stmt.borrow_mut();
//! let rs = stmt.execute_query(&vec![]).unwrap();
//! assert!(rs.as_ref().borrow_mut().next());
//! assert_eq!(Some(123), rs.as_ref().borrow_mut().get_i32(1));
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use fallible_streaming_iterator::FallibleStreamingIterator;
use rdbc;
use rusqlite::Rows;

/// Convert a Sqlite error into an RDBC error
fn to_rdbc_err(e: &rusqlite::Error) -> rdbc::Error {
    rdbc::Error::General(format!("{:?}", e))
}

pub struct SqliteDriver {}

impl SqliteDriver {
    pub fn new() -> Self {
        SqliteDriver {}
    }

    pub fn connect_in_memory(&self) -> rdbc::Result<Rc<RefCell<dyn rdbc::Connection>>> {
        rusqlite::Connection::open_in_memory()
            .map_err(|e| to_rdbc_err(&e))
            .map(|c| {
                Ok(Rc::new(RefCell::new(SConnection::new(c))) as Rc<RefCell<dyn rdbc::Connection>>)
            })?
    }
}

struct SConnection {
    conn: rusqlite::Connection,
}

impl SConnection {
    pub fn new(conn: rusqlite::Connection) -> Self {
        Self { conn }
    }
}

impl rdbc::Connection for SConnection {
    fn create(&mut self, sql: &str) -> rdbc::Result<Rc<RefCell<dyn rdbc::Statement + '_>>> {
        self.prepare(sql)
    }

    fn prepare(&mut self, sql: &str) -> rdbc::Result<Rc<RefCell<dyn rdbc::Statement + '_>>> {
        let stmt = self.conn.prepare(sql).map_err(|e| to_rdbc_err(&e))?;
        Ok(Rc::new(RefCell::new(SStatement { stmt })) as Rc<RefCell<dyn rdbc::Statement>>)
    }
}

struct SStatement<'a> {
    stmt: rusqlite::Statement<'a>,
}

impl<'a> rdbc::Statement for SStatement<'a> {
    fn execute_query(
        &mut self,
        params: &[rdbc::Value],
    ) -> rdbc::Result<Rc<RefCell<dyn rdbc::ResultSet + '_>>> {
        let params = to_sqlite_value(params);
        let params: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|v| v.as_ref()).collect();
        let rows = self.stmt.query(&params).map_err(|e| to_rdbc_err(&e))?;
        return Ok(Rc::new(RefCell::new(SResultSet { rows })) as Rc<RefCell<dyn rdbc::ResultSet>>);
    }

    fn execute_update(&mut self, params: &[rdbc::Value]) -> rdbc::Result<u64> {
        let params = to_sqlite_value(params);
        let params: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|v| v.as_ref()).collect();
        return self
            .stmt
            .execute(&params)
            .map_err(|e| to_rdbc_err(&e))
            .map(|n| n as u64);
    }
}

struct SResultSet<'stmt> {
    rows: Rows<'stmt>,
}

impl<'stmt> rdbc::ResultSet for SResultSet<'stmt> {
    fn meta_data(&self) -> rdbc::Result<Rc<dyn rdbc::ResultSetMetaData>> {
        let meta: Vec<rdbc::Column> = self
            .rows
            .columns()
            .unwrap()
            .iter()
            .map(|c| rdbc::Column::new(c.name(), to_rdbc_type(c.decl_type())))
            .collect();
        Ok(Rc::new(meta))
    }

    fn next(&mut self) -> bool {
        self.rows.next().unwrap().is_some()
    }

    fn get_i32(&self, i: u64) -> Option<i32> {
        self.rows.get().unwrap().get(i as usize - 1).ok()
    }

    fn get_string(&self, i: u64) -> Option<String> {
        self.rows.get().unwrap().get(i as usize - 1).ok()
    }
}

fn to_rdbc_type(t: Option<&str>) -> rdbc::DataType {
    //TODO implement for real
    match t {
        Some("INT") => rdbc::DataType::Integer,
        _ => rdbc::DataType::Varchar,
    }
}

fn to_sqlite_value(values: &[rdbc::Value]) -> Vec<Box<dyn rusqlite::types::ToSql>> {
    values
        .iter()
        .map(|v| match v {
            rdbc::Value::String(s) => Box::new(s.clone()) as Box<dyn rusqlite::types::ToSql>,
            rdbc::Value::Int32(n) => Box::new(*n) as Box<dyn rusqlite::types::ToSql>,
            rdbc::Value::UInt32(n) => Box::new(*n) as Box<dyn rusqlite::types::ToSql>,
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use rdbc::{Connection, DataType};

    #[test]
    fn execute_query() -> rdbc::Result<()> {
        let driver = SqliteDriver::new();
        let mut conn = driver.connect_in_memory()?;
        execute(&mut conn, "DROP TABLE IF EXISTS test", &vec![])?;
        execute(&mut conn, "CREATE TABLE test (a INT NOT NULL)", &vec![])?;
        execute(
            &mut conn,
            "INSERT INTO test (a) VALUES (?)",
            &vec![rdbc::Value::Int32(123)],
        )?;

        let mut conn = conn.as_ref().borrow_mut();
        let stmt = conn.prepare("SELECT a FROM test")?;
        let mut stmt = stmt.borrow_mut();
        let rs = stmt.execute_query(&vec![])?;

        let mut rs = rs.as_ref().borrow_mut();

        let meta = rs.meta_data().unwrap();
        assert_eq!(1, meta.num_columns());
        assert_eq!("a".to_owned(), meta.column_name(1));
        assert_eq!(DataType::Integer, meta.column_type(1));

        assert!(rs.next());
        assert_eq!(Some(123), rs.get_i32(1));
        assert!(!rs.next());

        Ok(())
    }

    fn execute(
        conn: &mut Rc<RefCell<dyn Connection>>,
        sql: &str,
        values: &Vec<rdbc::Value>,
    ) -> rdbc::Result<u64> {
        println!("Executing '{}' with {} params", sql, values.len());
        let mut conn = conn.as_ref().borrow_mut();
        let stmt = conn.prepare(sql)?;
        let mut stmt = stmt.borrow_mut();
        stmt.execute_update(values)
    }
}
