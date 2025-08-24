import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { readFile, writeFile } from "@tauri-apps/plugin-fs";

import "./App.css";

type EntryListItem = {
  id: number;
  site: string;
  username: string;
  created_at: number;
  updated_at: number;
};

type Entry = {
  id: number;
  site: string;
  username: string;
  password: string;
  notes?: string | null;
  created_at: number;
  updated_at: number;
};

function fmt(ts: number) {
  const d = new Date(ts * 1000);
  return d.toLocaleString();
}

// Универсальная обёртка, чтобы не терять ошибки
async function call<T>(cmd: string, args?: any): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e: any) {
    const msg = typeof e === "string" ? e : e?.message ?? JSON.stringify(e);
    alert(`Error: ${msg}`);
    throw e;
  }
}

export default function App() {
  const [unlocked, setUnlocked] = useState(false);
  const [master, setMaster] = useState("");
  const [search, setSearch] = useState("");
  const [items, setItems] = useState<EntryListItem[]>([]);
  const [selected, setSelected] = useState<Entry | null>(null);

  // контролируемые поля для генерации паролей
  const [addPwd, setAddPwd] = useState("");
  const [editPwd, setEditPwd] = useState("");

  const reload = async () => {
    const ok = await call<boolean>("vault_is_unlocked");
    setUnlocked(ok);
    if (ok) {
      const list = await call<EntryListItem[]>("list_entries", {
        search: search.trim() ? search : null,
      });
      setItems(list);
    } else {
      setItems([]);
      setSelected(null);
    }
  };

  useEffect(() => {
    reload();
  }, []);

  const handleInit = async () => {
    if (!master.trim()) return;
    try {
      await call("vault_init", { master });
    } catch (e: any) {
      const msg = String(typeof e === "string" ? e : e?.message ?? "");
      if (msg.toLowerCase().includes("already initialized")) {
        await call("vault_unlock", { master });
      } else {
        throw e;
      }
    }
    setMaster("");
    await reload();
  };

  const handleUnlock = async () => {
    if (!master.trim()) return;
    await call("vault_unlock", { master });
    setMaster("");
    await reload();
  };

  const handleLock = async () => {
    await call("vault_lock");
    await reload();
  };

  const handleAdd = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const fd = new FormData(e.currentTarget);
    const site = String(fd.get("site") || "");
    const username = String(fd.get("username") || "");
    const password = String(fd.get("password") || "");
    const notes = String(fd.get("notes") || "");
    const id = await call<number>("add_entry", {
      site,
      username,
      password,
      notes: notes.trim() ? notes : null,
    });
    (e.target as HTMLFormElement).reset();
    setAddPwd("");
    setSearch("");
    await reload();
    // auto-open
    const ent = await call<Entry>("get_entry", { id });
    setSelected(ent);
  };

  const openEntry = async (id: number) => {
    const ent = await call<Entry>("get_entry", { id });
    setSelected(ent);
    setEditPwd(""); // сбрасываем поле нового пароля
  };

  const handleDelete = async (id: number) => {
    if (!confirm("Delete entry?")) return;
    await call("delete_entry", { id });
    await reload();
  };

  const handleUpdate = async (
    e: React.FormEvent<HTMLFormElement>,
    id: number
  ) => {
    e.preventDefault();
    const fd = new FormData(e.currentTarget);
    const site = String(fd.get("site") || "");
    const username = String(fd.get("username") || "");
    const password = String(fd.get("password") || "");
    const notes = String(fd.get("notes") || "");
    await call("update_entry", {
      id,
      site,
      username,
      password: password ? password : null,
      notes: notes.length ? notes : null,
    });
    await reload();
    const ent = await call<Entry>("get_entry", { id });
    setSelected(ent);
    setEditPwd("");
  };

  const doSearch = async (e: React.FormEvent) => {
    e.preventDefault();
    await reload();
  };

  const generate = async (
    len = 20,
    set?: (s: string) => void
  ) => {
    const s = await call<string>("generate_password", {
      length: len,
      useDigits: true,
      useUpper: true,
      useSymbols: true,
    });
    set?.(s);
    try {
      await navigator.clipboard.writeText(s);
      alert("Generated password copied to clipboard");
    } catch {
      // молча игнорируем, если нет разрешения на буфер обмена
    }
  };

const exportBackup = async () => {
  const path = await save({ defaultPath: "backup.vault" });
  if (!path) return;
  // получаем зашифрованные байты с бэкапом
  const bytes = await call<Uint8Array>("export_backup_bytes");
  await writeFile(path, bytes);
  alert("Exported");
};

const importBackup = async () => {
  const path = await open({
    multiple: false,
    filters: [{ name: "Vault", extensions: ["vault"] }],
  });
  if (!path || Array.isArray(path)) return;
  const bytes = await readFile(path);
  // invoke лучше кормить обычным массивом чисел
  const count = await call<number>("import_backup_bytes", { data: Array.from(bytes) });
  alert(`Imported ${count} entries`);
  await reload();
};

  const left = useMemo(() => {
    if (!unlocked) return null;
    return (
      <div className="card">
        <form onSubmit={doSearch} className="row">
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search site or username"
          />
          <button type="submit">Search</button>
          <button
            type="button"
            onClick={() => {
              setSearch("");
              reload();
            }}
          >
            Reset
          </button>
        </form>

        <div className="list">
          {items.map((it) => (
            <div key={it.id} className="item">
              <div className="item-main" onClick={() => openEntry(it.id)}>
                <b>{it.site}</b>
                <div className="muted">{it.username}</div>
                <div className="muted small">{fmt(it.updated_at)}</div>
              </div>
              <div className="item-actions">
                <button onClick={() => openEntry(it.id)}>Open</button>
                <button onClick={() => handleDelete(it.id)}>Delete</button>
              </div>
            </div>
          ))}
          {items.length === 0 && <div className="muted">No items</div>}
        </div>
      </div>
    );
  }, [unlocked, items, search]);

  const right = useMemo(() => {
    if (!unlocked) return null;
    return (
      <div className="card">
        <h2>Add entry</h2>
        <form onSubmit={handleAdd} className="col">
          <input name="site" placeholder="Site (e.g. example.com)" required />
          <input name="username" placeholder="Username" required />
          <div className="row">
            <input
              name="password"
              placeholder="Password"
              required
              value={addPwd}
              onChange={(e) => setAddPwd(e.target.value)}
            />
            <button type="button" onClick={() => generate(20, setAddPwd)}>
              Generate
            </button>
          </div>
          <textarea name="notes" placeholder="Notes (optional)" />
          <button type="submit">Add</button>
        </form>

        {selected && (
          <>
            <h2>Edit entry</h2>
            <form onSubmit={(e) => handleUpdate(e, selected.id)} className="col">
              <input name="site" defaultValue={selected.site} required />
              <input name="username" defaultValue={selected.username} required />
              <div className="row">
                <input
                  name="password"
                  placeholder="New password (optional)"
                  value={editPwd}
                  onChange={(e) => setEditPwd(e.target.value)}
                />
                <button
                  type="button"
                  onClick={() => generate(20, setEditPwd)}
                >
                  Generate
                </button>
              </div>
              <textarea name="notes" defaultValue={selected.notes ?? ""} />
              <button type="submit">Save</button>
            </form>

            <h3>Decrypted password</h3>
            <code style={{ userSelect: "all" }}>{selected.password}</code>
          </>
        )}
      </div>
    );
  }, [unlocked, selected, addPwd, editPwd]);

  return (
    <main className="container">
      <h1>Password Vault</h1>

      {!unlocked ? (
        <div className="card">
          <h2>Setup / Unlock</h2>
          <div className="row">
            <input
              type="password"
              value={master}
              onChange={(e) => setMaster(e.target.value)}
              placeholder="Master password"
            />
            <button onClick={handleInit}>Init</button>
            <button onClick={handleUnlock}>Unlock</button>
          </div>
          <p className="muted">
            New install? Click <b>Init</b>. Otherwise enter your master password
            and click <b>Unlock</b>.
          </p>
        </div>
      ) : (
        <div className="row">
          <button onClick={handleLock}>Lock</button>
          <button onClick={exportBackup}>Export</button>
          <button onClick={importBackup}>Import</button>
        </div>
      )}

      <div className="grid">
        {left}
        {right}
      </div>
    </main>
  );
}
