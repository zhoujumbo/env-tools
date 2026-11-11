use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const PROTECTED: &[&str] = &[
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "NUMBER_OF_PROCESSORS",
    "PROMPT",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvChange {
    pub name: String,
    /// set | delete
    pub action: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSnapshot {
    pub user: Vec<EnvVar>,
    pub system: Vec<EnvVar>,
}

pub fn is_protected(name: &str) -> bool {
    PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(name))
}

pub fn snapshot() -> EnvSnapshot {
    let to_vec = |m: HashMap<String, String>| {
        let mut v: Vec<EnvVar> = m
            .into_iter()
            .map(|(name, value)| EnvVar { name, value })
            .collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        v
    };
    EnvSnapshot {
        user: to_vec(crate::winutil::read_env_map(false).unwrap_or_default()),
        system: to_vec(crate::winutil::read_env_map(true).unwrap_or_default()),
    }
}

pub fn apply_user_changes(changes: &[EnvChange]) -> Result<(serde_json::Value, serde_json::Value)> {
    let mut before = serde_json::Map::new();
    let mut after = serde_json::Map::new();
    let mut seen = HashSet::new();
    for ch in changes {
        if is_protected(&ch.name) {
            bail!("拒绝修改受保护变量 {}", ch.name);
        }
        if !seen.insert(ch.name.to_uppercase()) {
            continue;
        }
        let prev = crate::winutil::get_user_env(&ch.name)?;
        before.insert(
            ch.name.clone(),
            prev.clone().map(serde_json::Value::String).unwrap_or(serde_json::Value::Null),
        );
        match ch.action.as_str() {
            "delete" => {
                crate::winutil::delete_user_env(&ch.name)?;
                after.insert(ch.name.clone(), serde_json::Value::Null);
            }
            "set" => {
                let v = ch.value.clone().unwrap_or_default();
                crate::winutil::set_user_env(&ch.name, &v)?;
                after.insert(ch.name.clone(), serde_json::Value::String(v));
            }
            other => bail!("未知 action: {other}"),
        }
    }
    crate::winutil::broadcast_environment();
    Ok((serde_json::Value::Object(before), serde_json::Value::Object(after)))
}

pub fn undo_env_map(before: &serde_json::Value) -> Result<()> {
    let Some(obj) = before.as_object() else {
        return Ok(());
    };
    for (k, v) in obj {
        if is_protected(k) {
            continue;
        }
        match v {
            serde_json::Value::Null => crate::winutil::delete_user_env(k)?,
            serde_json::Value::String(s) => crate::winutil::set_user_env(k, s)?,
            _ => {}
        }
    }
    crate::winutil::broadcast_environment();
    Ok(())
}
