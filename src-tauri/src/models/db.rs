use rusqlite::{params, Connection, Result};
pub struct DataBase {

}


impl DataBase {
    pub fn connect() -> Result<()> {
        let db = Connection::open("password_vault.db");
        println!("LOG: Connected to DB");
        Ok(())

    }
}