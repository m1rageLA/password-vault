use std::fs;

use rusqlite::{ffi::sqlite3_sql, params, Connection, Result};

pub struct DataBase {
    db: Connection,
}

impl DataBase {
    pub fn open(path: &str) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS PASSWORDS (id INTEGER PRIMARY KEY AUTOINCREMENT, password TEXT NOT NULL);",
        )?;
        println!("LOG: Connected to DB");
        Ok(Self { db })
    }

    pub fn add_password(self, password: &str) -> Result<i64> {
        self.db.execute(
            "INSERT INTO PASSWORDS (password) VALUES (?1)",
            params![password],
        )?;
        Ok(self.db.last_insert_rowid())
    }
}
