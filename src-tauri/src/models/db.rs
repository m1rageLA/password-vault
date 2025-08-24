use std::{path::Path, str::FromStr, sync::Arc};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use getrandom::getrandom;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    Executor, Row,
};
use thiserror::Error;
use tokio::sync::RwLock;
use zeroize::Zeroize;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("vault not initialized with a master password")]
    NotInitialized,
    #[error("vault already initialized")]
    AlreadyInitialized,
    #[error("vault is locked")]
    Locked,
    #[error("invalid master password")]
    BadMasterPassword,
    #[error("crypto error")]
    Crypto,
    #[error("{0}")]
    Other(String),
}

type ResultT<T> = Result<T, VaultError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KdfParams {
    mem_cost_kib: u32,
    iterations: u32,
    parallelism: u32,
}
impl Default for KdfParams {
    fn default() -> Self {
        Self {
            mem_cost_kib: 19_456,
            iterations: 2,
            parallelism: 1,
        }
    }
}
fn argon2_from_params(p: &KdfParams) -> Argon2<'static> {
    let params = Params::new(p.mem_cost_kib, p.iterations, p.parallelism, None).unwrap();
    Argon2::new_with_secret(&[], Algorithm::Argon2id, Version::V0x13, params).unwrap()
}

/// nonce||ciphertext
fn encrypt(key_bytes: &[u8; 32], plaintext: &[u8]) -> ResultT<Vec<u8>> {
    let key = Key::from_slice(key_bytes);
    let cipher = ChaCha20Poly1305::new(key);

    let mut nonce_bytes = [0u8; 12];
    getrandom(&mut nonce_bytes).map_err(|_| VaultError::Crypto)?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let mut out = nonce_bytes.to_vec();
    let ct = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| VaultError::Crypto)?;
    out.extend_from_slice(&ct);
    Ok(out)
}

/// ожидает nonce||ciphertext
fn decrypt(key_bytes: &[u8; 32], data: &[u8]) -> ResultT<Vec<u8>> {
    if data.len() < 12 {
        return Err(VaultError::Crypto);
    }
    let (nonce_bytes, ct) = data.split_at(12);
    let key = Key::from_slice(key_bytes);
    let cipher = ChaCha20Poly1305::new(key);
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ct)
        .map_err(|_| VaultError::BadMasterPassword)
}

const KEY_CHECK_PLAINTEXT: &[u8] = b"vault-key-check";

#[derive(Clone)]
pub struct DataBase {
    pool: SqlitePool,
    key: Arc<RwLock<Option<[u8; 32]>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub site: String,
    pub username: String,
    pub password: String,
    pub notes: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryListItem {
    pub id: i64,
    pub site: String,
    pub username: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl DataBase {
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self, sqlx::Error> {
        let uri = format!("sqlite://{}", path.as_ref().to_string_lossy());
        let opts = SqliteConnectOptions::from_str(&uri)?.create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        pool.execute(
            r#"
            PRAGMA journal_mode=WAL;

            CREATE TABLE IF NOT EXISTS vault_config (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                kdf_salt BLOB NOT NULL,
                kdf_params TEXT NOT NULL,
                key_check BLOB NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                site TEXT NOT NULL,
                username TEXT NOT NULL,
                password_enc BLOB NOT NULL,
                notes_enc BLOB,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_entries_site ON entries(site);
            CREATE INDEX IF NOT EXISTS idx_entries_username ON entries(username);
            "#,
        )
        .await?;

        Ok(Self {
            pool,
            key: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn init_master(&self, master: SecretString) -> ResultT<()> {
        let row = sqlx::query("SELECT COUNT(*) as c FROM vault_config")
            .fetch_one(&self.pool)
            .await?;
        if row.get::<i64, _>("c") > 0 {
            return Err(VaultError::AlreadyInitialized);
        }

        let mut salt = [0u8; 16];
        getrandom(&mut salt).map_err(|_| VaultError::Crypto)?;

        let kdf_params = KdfParams::default();
        let argon = argon2_from_params(&kdf_params);

        let mut key = [0u8; 32];
        argon
            .hash_password_into(master.expose_secret().as_bytes(), &salt, &mut key)
            .map_err(|_| VaultError::Crypto)?;

        let key_check = encrypt(&key, KEY_CHECK_PLAINTEXT)?;
        let now = epoch();

        sqlx::query(
            "INSERT INTO vault_config (id, kdf_salt, kdf_params, key_check, created_at)
             VALUES (1, ?, ?, ?, ?)",
        )
        .bind(salt.to_vec())
        .bind(serde_json::to_string(&kdf_params).unwrap())
        .bind(key_check)
        .bind(now)
        .execute(&self.pool)
        .await?;

        *self.key.write().await = Some(key);
        Ok(())
    }

    pub async fn unlock(&self, master: SecretString) -> ResultT<()> {
        let row =
            sqlx::query("SELECT kdf_salt, kdf_params, key_check FROM vault_config WHERE id=1")
                .fetch_optional(&self.pool)
                .await?
                .ok_or(VaultError::NotInitialized)?;

        let salt: Vec<u8> = row.get("kdf_salt");
        let kdf_params: KdfParams = serde_json::from_str(&row.get::<String, _>("kdf_params"))
            .map_err(|e| VaultError::Other(e.to_string()))?;
        let key_check: Vec<u8> = row.get("key_check");

        let argon = argon2_from_params(&kdf_params);
        let mut key = [0u8; 32];
        argon
            .hash_password_into(master.expose_secret().as_bytes(), &salt, &mut key)
            .map_err(|_| VaultError::Crypto)?;

        let check_plain = decrypt(&key, &key_check)?;
        if check_plain != KEY_CHECK_PLAINTEXT {
            key.zeroize();
            return Err(VaultError::BadMasterPassword);
        }

        *self.key.write().await = Some(key);
        Ok(())
    }

    pub async fn lock(&self) {
        if let Some(mut k) = self.key.write().await.take() {
            k.zeroize();
        }
    }
    pub async fn is_unlocked(&self) -> bool {
        self.key.read().await.is_some()
    }

    pub async fn add_entry(
        &self,
        site: &str,
        username: &str,
        password: &str,
        notes: Option<&str>,
    ) -> ResultT<i64> {
        let key = self.get_key().await?;
        let now = epoch();

        let pwd_ct = encrypt(&key, password.as_bytes())?;
        let notes_ct = match notes {
            Some(n) if !n.is_empty() => Some(encrypt(&key, n.as_bytes())?),
            _ => None,
        };

        let res = sqlx::query(
            "INSERT INTO entries (site, username, password_enc, notes_enc, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(site)
        .bind(username)
        .bind(pwd_ct)
        .bind(notes_ct)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(res.last_insert_rowid())
    }

    pub async fn get_entry(&self, id: i64) -> ResultT<Entry> {
        let key = self.get_key().await?;
        let row = sqlx::query(
            "SELECT id, site, username, password_enc, notes_enc, created_at, updated_at
             FROM entries WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        let pwd_ct: Vec<u8> = row.get("password_enc");
        let password =
            String::from_utf8(decrypt(&key, &pwd_ct)?).map_err(|_| VaultError::Crypto)?;
        let notes = match row.try_get::<Vec<u8>, _>("notes_enc") {
            Ok(ct) => Some(String::from_utf8(decrypt(&key, &ct)?).map_err(|_| VaultError::Crypto)?),
            Err(_) => None,
        };

        Ok(Entry {
            id: row.get("id"),
            site: row.get("site"),
            username: row.get("username"),
            password,
            notes,
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
    }

    pub async fn list_entries(&self, search: Option<&str>) -> ResultT<Vec<EntryListItem>> {
        let like = search.map(|s| format!("%{}%", s.trim()));
        let rows = if let Some(l) = like {
            sqlx::query(
                "SELECT id, site, username, created_at, updated_at
                 FROM entries
                 WHERE site LIKE ? OR username LIKE ?
                 ORDER BY updated_at DESC, id DESC",
            )
            .bind(&l)
            .bind(&l)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, site, username, created_at, updated_at
                 FROM entries
                 ORDER BY updated_at DESC, id DESC",
            )
            .fetch_all(&self.pool)
            .await?
        };
        Ok(rows
            .into_iter()
            .map(|r| EntryListItem {
                id: r.get("id"),
                site: r.get("site"),
                username: r.get("username"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    pub async fn update_entry(
        &self,
        id: i64,
        site: &str,
        username: &str,
        password: Option<&str>,
        notes: Option<&str>,
    ) -> ResultT<()> {
        let key = self.get_key().await?;
        let now = epoch();

        let row = sqlx::query("SELECT password_enc, notes_enc FROM entries WHERE id=?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        let mut pwd_ct: Vec<u8> = row.get("password_enc");
        let mut notes_ct: Option<Vec<u8>> = row.try_get("notes_enc").ok();

        if let Some(p) = password {
            pwd_ct = encrypt(&key, p.as_bytes())?;
        }
        if let Some(n) = notes {
            notes_ct = if n.is_empty() {
                None
            } else {
                Some(encrypt(&key, n.as_bytes())?)
            };
        }

        sqlx::query(
            "UPDATE entries SET site=?, username=?, password_enc=?, notes_enc=?, updated_at=? WHERE id=?"
        )
        .bind(site).bind(username).bind(pwd_ct).bind(notes_ct).bind(now).bind(id)
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn delete_entry(&self, id: i64) -> ResultT<()> {
        sqlx::query("DELETE FROM entries WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn export_encrypted_bytes(&self) -> ResultT<Vec<u8>> {
        let key = self.get_key().await?;
        let rows = sqlx::query(
        "SELECT id, site, username, password_enc, notes_enc, created_at, updated_at FROM entries"
    ).fetch_all(&self.pool).await?;

        #[derive(Serialize)]
        struct Plain {
            id: i64,
            site: String,
            username: String,
            password: String,
            notes: Option<String>,
            created_at: i64,
            updated_at: i64,
        }

        let mut items = Vec::with_capacity(rows.len());
        for r in rows {
            let pwd_ct: Vec<u8> = r.get("password_enc");
            let password =
                String::from_utf8(decrypt(&key, &pwd_ct)?).map_err(|_| VaultError::Crypto)?;
            let notes = match r.try_get::<Vec<u8>, _>("notes_enc") {
                Ok(ct) => {
                    Some(String::from_utf8(decrypt(&key, &ct)?).map_err(|_| VaultError::Crypto)?)
                }
                Err(_) => None,
            };
            items.push(Plain {
                id: r.get("id"),
                site: r.get("site"),
                username: r.get("username"),
                password,
                notes,
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            });
        }

        let json = serde_json::to_vec(&items).unwrap();
        let sealed = encrypt(&key, &json)?;
        Ok(sealed)
    }

    pub async fn import_encrypted_bytes(&self, data: &[u8]) -> ResultT<usize> {
        let key = self.get_key().await?;
        let plain = decrypt(&key, data)?;

        #[derive(Deserialize)]
        struct Plain {
            site: String,
            username: String,
            password: String,
            notes: Option<String>,
        }
        let items: Vec<Plain> =
            serde_json::from_slice(&plain).map_err(|e| VaultError::Other(e.to_string()))?;

        let mut count = 0usize;
        for it in items {
            self.add_entry(&it.site, &it.username, &it.password, it.notes.as_deref())
                .await?;
            count += 1;
        }
        Ok(count)
    }

    pub async fn export_encrypted_backup<P: AsRef<Path>>(&self, path: P) -> ResultT<()> {
        let key = self.get_key().await?;
        let rows = sqlx::query(
            "SELECT id, site, username, password_enc, notes_enc, created_at, updated_at FROM entries"
        ).fetch_all(&self.pool).await?;

        #[derive(Serialize)]
        struct Plain {
            id: i64,
            site: String,
            username: String,
            password: String,
            notes: Option<String>,
            created_at: i64,
            updated_at: i64,
        }

        let mut items = Vec::with_capacity(rows.len());
        for r in rows {
            let pwd_ct: Vec<u8> = r.get("password_enc");
            let password =
                String::from_utf8(decrypt(&key, &pwd_ct)?).map_err(|_| VaultError::Crypto)?;
            let notes = match r.try_get::<Vec<u8>, _>("notes_enc") {
                Ok(ct) => {
                    Some(String::from_utf8(decrypt(&key, &ct)?).map_err(|_| VaultError::Crypto)?)
                }
                Err(_) => None,
            };
            items.push(Plain {
                id: r.get("id"),
                site: r.get("site"),
                username: r.get("username"),
                password,
                notes,
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            });
        }

        let json = serde_json::to_vec(&items).unwrap();
        let sealed = encrypt(&key, &json)?;
        std::fs::write(path, sealed).map_err(|e| VaultError::Other(e.to_string()))
    }

    pub async fn import_encrypted_backup<P: AsRef<Path>>(&self, path: P) -> ResultT<usize> {
        let key = self.get_key().await?;
        let bytes = std::fs::read(path).map_err(|e| VaultError::Other(e.to_string()))?;
        let plain = decrypt(&key, &bytes)?;

        #[derive(Deserialize)]
        struct Plain {
            site: String,
            username: String,
            password: String,
            notes: Option<String>,
        }
        let items: Vec<Plain> =
            serde_json::from_slice(&plain).map_err(|e| VaultError::Other(e.to_string()))?;

        let mut count = 0usize;
        for it in items {
            self.add_entry(&it.site, &it.username, &it.password, it.notes.as_deref())
                .await?;
            count += 1;
        }
        Ok(count)
    }

    pub fn generate_password(
        &self,
        length: usize,
        use_digits: bool,
        use_upper: bool,
        use_symbols: bool,
    ) -> String {
        let mut charset = String::from("abcdefghijklmnopqrstuvwxyz");
        if use_upper {
            charset.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        }
        if use_digits {
            charset.push_str("0123456789");
        }
        if use_symbols {
            charset.push_str("!@#$%^&*()-_=+[]{};:,.?/\\|`~");
        }
        let bytes = charset.as_bytes();
        if bytes.is_empty() {
            return String::new();
        }

        let mut out = String::with_capacity(length);
        for _ in 0..length {
            let mut r = [0u8; 8];
            // криптографически стойкий источник
            getrandom(&mut r).expect("os rng");
            let idx = (u64::from_le_bytes(r) as usize) % bytes.len();
            out.push(bytes[idx] as char);
        }
        out
    }

    pub async fn get_password(&self, id: i64) -> ResultT<String> {
        Ok(self.get_entry(id).await?.password)
    }

    async fn get_key(&self) -> ResultT<[u8; 32]> {
        self.key
            .read()
            .await
            .as_ref()
            .copied()
            .ok_or(VaultError::Locked)
    }
}

fn epoch() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn end_to_end() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("t.db");
        let db = DataBase::open(&db_path).await.unwrap();

        db.init_master(SecretString::new("master123".into()))
            .await
            .unwrap();
        assert!(db.is_unlocked().await);

        let id = db
            .add_entry("example.com", "alice", "p@ss", Some("note"))
            .await
            .unwrap();
        let e = db.get_entry(id).await.unwrap();
        assert_eq!(e.username, "alice");
        assert_eq!(e.password, "p@ss");
        assert_eq!(e.notes.as_deref(), Some("note"));

        db.update_entry(id, "example.org", "alice", Some("new"), None)
            .await
            .unwrap();
        let e2 = db.get_entry(id).await.unwrap();
        assert_eq!(e2.site, "example.org");
        assert_eq!(e2.password, "new");
        assert!(e2.notes.is_none());

        let all = db.list_entries(None).await.unwrap();
        assert_eq!(all.len(), 1);
        let found = db.list_entries(Some("example")).await.unwrap();
        assert_eq!(found.len(), 1);

        let backup = dir.path().join("b.vault");
        db.export_encrypted_backup(&backup).await.unwrap();
        let imported = db.import_encrypted_backup(&backup).await.unwrap();
        assert!(imported >= 1);

        db.lock().await;
        assert!(!db.is_unlocked().await);
        assert!(matches!(db.get_entry(id).await, Err(VaultError::Locked)));
        db.unlock(SecretString::new("master123".into()))
            .await
            .unwrap();
        let _ = db.get_entry(id).await.unwrap();
    }
}
