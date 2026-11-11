use crate::paths::AppPaths;
use crate::winutil::{self, CmdResult};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpRecord {
    pub id: String,
    pub ts: String,
    pub actor: String,
    pub kind: String,
    pub cmdline: Option<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub undo_json: Option<String>,
    pub ok: bool,
}

pub struct Db(pub Mutex<Connection>);

impl Db {
    /// 锁中毒也继续用（panic = "abort" 下这条路径基本走不到，但别让一次
    /// 历史 panic 把整个日志/设置写入永久卡死）。
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        match self.0.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    pub fn open(paths: &AppPaths) -> Result<Self> {
        let conn = Connection::open(paths.db_path())?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA busy_timeout=3000;
            CREATE TABLE IF NOT EXISTS ops (
              id TEXT PRIMARY KEY,
              ts TEXT NOT NULL,
              actor TEXT NOT NULL,
              kind TEXT NOT NULL,
              cmdline TEXT,
              cwd TEXT,
              exit_code INTEGER,
              stdout_path TEXT,
              stderr_path TEXT,
              before_json TEXT,
              after_json TEXT,
              undo_json TEXT,
              ok INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_ops_kind_ts ON ops(kind, ts);
            "#,
        )?;
        // 日志只留最近 3000 条：面板是长期常驻的，ops 表无限长下去只是拖慢启动查询。
        let _ = conn.execute(
            "DELETE FROM ops WHERE id NOT IN (SELECT id FROM ops ORDER BY ts DESC LIMIT 3000)",
            [],
        );
        Ok(Self(Mutex::new(conn)))
    }

    pub fn reopen(&self, paths: &AppPaths) -> Result<()> {
        let conn = Connection::open(paths.db_path())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=3000;",
        )?;
        *self.conn() = conn;
        Ok(())
    }

    pub fn insert(&self, rec: &OpRecord) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            r#"INSERT INTO ops
            (id, ts, actor, kind, cmdline, cwd, exit_code, stdout_path, stderr_path,
             before_json, after_json, undo_json, ok)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)"#,
            params![
                rec.id,
                rec.ts,
                rec.actor,
                rec.kind,
                rec.cmdline,
                rec.cwd,
                rec.exit_code,
                rec.stdout_path,
                rec.stderr_path,
                rec.before_json,
                rec.after_json,
                rec.undo_json,
                rec.ok as i32
            ],
        )?;
        Ok(())
    }

    pub fn list(&self, kind: Option<&str>, limit: i64) -> Result<Vec<OpRecord>> {
        let conn = self.conn();
        let mut rows = Vec::new();
        if let Some(k) = kind {
            let mut stmt = conn.prepare(
                "SELECT id, ts, actor, kind, cmdline, cwd, exit_code, stdout_path, stderr_path,
                        before_json, after_json, undo_json, ok
                 FROM ops WHERE kind = ?1 ORDER BY ts DESC LIMIT ?2",
            )?;
            let iter = stmt.query_map(params![k, limit], map_op)?;
            for r in iter {
                rows.push(r?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, ts, actor, kind, cmdline, cwd, exit_code, stdout_path, stderr_path,
                        before_json, after_json, undo_json, ok
                 FROM ops ORDER BY ts DESC LIMIT ?1",
            )?;
            let iter = stmt.query_map(params![limit], map_op)?;
            for r in iter {
                rows.push(r?);
            }
        }
        Ok(rows)
    }

    pub fn list_with_undo(&self) -> Result<Vec<OpRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, ts, actor, kind, cmdline, cwd, exit_code, stdout_path, stderr_path,
                    before_json, after_json, undo_json, ok
             FROM ops WHERE undo_json IS NOT NULL AND undo_json != '' AND ok = 1
             ORDER BY ts DESC",
        )?;
        let iter = stmt.query_map([], map_op)?;
        let mut rows = Vec::new();
        for r in iter {
            rows.push(r?);
        }
        Ok(rows)
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn();
        let v: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn add_touched_env_key(&self, name: &str) -> Result<()> {
        let mut keys: Vec<String> = self
            .meta_get("touched_env_keys")?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if !keys.iter().any(|k| k.eq_ignore_ascii_case(name)) {
            keys.push(name.to_string());
            self.meta_set("touched_env_keys", &serde_json::to_string(&keys)?)?;
        }
        Ok(())
    }

    pub fn touched_env_keys(&self) -> Result<Vec<String>> {
        Ok(self
            .meta_get("touched_env_keys")?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default())
    }
}

fn map_op(r: &rusqlite::Row<'_>) -> rusqlite::Result<OpRecord> {
    let ok: i32 = r.get(12)?;
    Ok(OpRecord {
        id: r.get(0)?,
        ts: r.get(1)?,
        actor: r.get(2)?,
        kind: r.get(3)?,
        cmdline: r.get(4)?,
        cwd: r.get(5)?,
        exit_code: r.get(6)?,
        stdout_path: r.get(7)?,
        stderr_path: r.get(8)?,
        before_json: r.get(9)?,
        after_json: r.get(10)?,
        undo_json: r.get(11)?,
        ok: ok != 0,
    })
}

pub fn log_command(
    db: &Db,
    paths: &AppPaths,
    actor: &str,
    kind: &str,
    result: &CmdResult,
    before: Option<&serde_json::Value>,
    after: Option<&serde_json::Value>,
    undo: Option<&serde_json::Value>,
) -> Result<OpRecord> {
    let stdout_path = winutil::write_log_file(&paths.log_dir, "stdout", &result.stdout)
        .ok()
        .map(|p| p.display().to_string());
    let stderr_path = winutil::write_log_file(&paths.log_dir, "stderr", &result.stderr)
        .ok()
        .map(|p| p.display().to_string());
    let rec = OpRecord {
        id: Uuid::new_v4().to_string(),
        ts: chrono::Local::now().to_rfc3339(),
        actor: actor.into(),
        kind: kind.into(),
        cmdline: Some(result.cmdline.clone()),
        cwd: result.cwd.clone(),
        exit_code: Some(result.exit_code),
        stdout_path,
        stderr_path,
        before_json: before.map(|v| v.to_string()),
        after_json: after.map(|v| v.to_string()),
        undo_json: undo.map(|v| v.to_string()),
        ok: result.ok,
    };
    db.insert(&rec)?;
    Ok(rec)
}

pub fn log_action(
    db: &Db,
    actor: &str,
    kind: &str,
    cmdline: Option<String>,
    before: Option<&serde_json::Value>,
    after: Option<&serde_json::Value>,
    undo: Option<&serde_json::Value>,
    ok: bool,
) -> Result<OpRecord> {
    let rec = OpRecord {
        id: Uuid::new_v4().to_string(),
        ts: chrono::Local::now().to_rfc3339(),
        actor: actor.into(),
        kind: kind.into(),
        cmdline,
        cwd: None,
        exit_code: Some(if ok { 0 } else { 1 }),
        stdout_path: None,
        stderr_path: None,
        before_json: before.map(|v| v.to_string()),
        after_json: after.map(|v| v.to_string()),
        undo_json: undo.map(|v| v.to_string()),
        ok,
    };
    db.insert(&rec)?;
    Ok(rec)
}
