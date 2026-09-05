//! 配置持久化（config.json）与消息历史（SQLite）

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::proto::{ChatMessage, MessageKind};

#[derive(Debug, Clone)]
pub struct Config {
    /// 设备指纹，首次生成后不变
    pub device_id: String,
    pub device_name: String,
    pub http_port: u16,
    pub download_dir: PathBuf,
    /// 自动接收发来的文件（不加确认）
    pub auto_receive: bool,
}

impl Config {
    pub fn app_dir() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .context("无法定位用户配置目录")?
            .join("LocalTransfer");
        std::fs::create_dir_all(&base)?;
        Ok(base)
    }

    pub fn load_or_create() -> Result<Config> {
        let path = Self::app_dir()?.join("config.json");
        if path.exists() {
            let raw = std::fs::read_to_string(&path).context("读取 config.json 失败")?;
            // 容错：剥离 UTF-8 BOM（PowerShell 等工具写回时会加，serde_json 不认）
            let raw = raw.trim_start_matches('\u{feff}');
            let cfg: Config = serde_json::from_str(raw).context("解析 config.json 失败")?;
            return Ok(cfg);
        }
        let cfg = Config {
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: random_poetic_name(),
            http_port: crate::proto::DEFAULT_HTTP_PORT,
            download_dir: default_download_dir(),
            auto_receive: false,
        };
        cfg.save()?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::app_dir()?.join("config.json");
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, raw)?;
        Ok(())
    }
}

fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("LocalTransfer")
}

// serde 手写（含 PathBuf，保持文件简洁）
impl serde::Serialize for Config {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Config", 5)?;
        st.serialize_field("device_id", &self.device_id)?;
        st.serialize_field("device_name", &self.device_name)?;
        st.serialize_field("http_port", &self.http_port)?;
        st.serialize_field(
            "download_dir",
            &self.download_dir.to_string_lossy().to_string(),
        )?;
        st.serialize_field("auto_receive", &self.auto_receive)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            device_id: String,
            device_name: String,
            http_port: u16,
            download_dir: String,
            #[serde(default)]
            auto_receive: bool,
        }
        let raw = Raw::deserialize(d)?;
        Ok(Config {
            device_id: raw.device_id,
            device_name: raw.device_name,
            http_port: raw.http_port,
            download_dir: PathBuf::from(raw.download_dir),
            auto_receive: raw.auto_receive,
        })
    }
}

/// 首次启动的随机设备名：诗意形容词 + 的 + 自然意象（LocalSend 风格）。
/// 用 uuid 字节做随机源，避免引入 rand 依赖。
pub fn random_poetic_name() -> String {
    const ADJ: [&str; 32] = [
        "温柔的", "安静的", "快乐的", "勇敢的", "自由的", "神秘的", "优雅的", "活泼的", "沉静的",
        "明亮的", "柔软的", "轻盈的", "悠然的", "清澈的", "可爱的", "狡黠的", "坦率的", "浪漫的",
        "顽皮的", "认真的", "热烈的", "朦胧的", "顺风的", "发光的", "微笑的", "好奇的",
        "懒洋洋的", "慢悠悠的", "亮晶晶的", "毛茸茸的", "圆滚滚的", "慢半拍的",
    ];
    const NOUN: [&str; 32] = [
        "山雀", "鲸鱼", "萤火", "松鼠", "云雀", "海豚", "月光", "星河", "芦苇", "清泉", "晚风",
        "候鸟", "竹叶", "雪花", "灯塔", "小熊", "旅人", "橘猫", "白鹭", "远山", "湖泊", "松果",
        "蒲公英", "布谷鸟", "小雨滴", "贝壳", "枫叶", "流星", "麦浪", "溪水", "云朵", "海风",
    ];
    let b = uuid::Uuid::new_v4();
    let bytes = b.as_bytes();
    let adj = ADJ[bytes[0] as usize % ADJ.len()];
    let noun = NOUN[bytes[1] as usize % NOUN.len()];
    format!("{adj}{noun}")
}

// ---------------------------------------------------------------------------
// SQLite 消息历史
// ---------------------------------------------------------------------------

pub struct Store {
    conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    peer_id     TEXT NOT NULL,
    outgoing    INTEGER NOT NULL,          -- 1=发出 0=收到
    kind        INTEGER NOT NULL,          -- 0=text 1=file
    content     TEXT NOT NULL,             -- 文本内容或文件显示名
    file_path   TEXT,                      -- 接收方保存位置
    file_size   INTEGER,
    transfer_id TEXT,
    file_id     TEXT,                       -- 发送方分配的单文件 ID（匹配进度）
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_peer ON messages(peer_id, id);
CREATE TABLE IF NOT EXISTS peers (
    id        TEXT PRIMARY KEY,
    name      TEXT NOT NULL,
    last_seen INTEGER NOT NULL
);
"#;

impl Store {
    pub fn open() -> Result<Self> {
        let db = Config::app_dir()?.join("history.db");
        Self::open_at(db)
    }

    /// 测试/自定义位置用
    pub fn open_at(db: impl AsRef<Path>) -> Result<Self> {
        let db = db.as_ref();
        if let Some(parent) = db.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db).with_context(|| format!("打开 {} 失败", db.display()))?;
        // WAL：core 与 UI 各持一个连接并发读写
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn insert_text(&self, peer_id: &str, outgoing: bool, text: &str, created_at: i64) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO messages(peer_id, outgoing, kind, content, created_at) VALUES(?1,?2,0,?3,?4)",
                rusqlite::params![peer_id, outgoing as i64, text, created_at],
            )
            .context("写入消息失败")?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_file(
        &self,
        peer_id: &str,
        outgoing: bool,
        name: &str,
        size: u64,
        file_path: Option<&Path>,
        transfer_id: Option<&str>,
        file_id: Option<&str>,
        created_at: i64,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO messages(peer_id, outgoing, kind, content, file_path, file_size, transfer_id, file_id, created_at)
                 VALUES(?1,?2,1,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    peer_id,
                    outgoing as i64,
                    name,
                    file_path.map(|p| p.to_string_lossy().to_string()),
                    size as i64,
                    transfer_id,
                    file_id,
                    created_at
                ],
            )
            .context("写入文件消息失败")?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 更新文件消息的保存位置（接收完成时）
    pub fn update_file_path(&self, msg_id: i64, path: &Path) -> Result<()> {
        self.conn
            .execute(
                "UPDATE messages SET file_path=?1 WHERE id=?2",
                rusqlite::params![path.to_string_lossy().to_string(), msg_id],
            )
            .context("更新文件位置失败")?;
        Ok(())
    }

    /// 倒序取最近 limit 条，再翻回时间正序
    pub fn load_history(&self, peer_id: &str, limit: i64) -> Result<Vec<ChatMessage>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, outgoing, kind, content, file_path, file_size, transfer_id, file_id, created_at
                 FROM messages WHERE peer_id=?1 ORDER BY id DESC LIMIT ?2",
            )
            .context("查询历史失败")?;
        let rows = stmt.query_map(rusqlite::params![peer_id, limit], |r| {
            let id: i64 = r.get(0)?;
            let outgoing: i64 = r.get(1)?;
            let kind: i64 = r.get(2)?;
            let content: String = r.get(3)?;
            let file_path: Option<String> = r.get(4)?;
            let file_size: Option<i64> = r.get(5)?;
            let transfer_id: Option<String> = r.get(6)?;
            let file_id: Option<String> = r.get(7)?;
            let created_at: i64 = r.get(8)?;
            let kind = if kind == 0 {
                MessageKind::Text(content)
            } else {
                MessageKind::File {
                    name: content,
                    size: file_size.unwrap_or(0) as u64,
                    saved_path: file_path.map(PathBuf::from),
                    transfer_id,
                    file_id,
                }
            };
            Ok(ChatMessage {
                id,
                peer_id: peer_id.to_string(),
                outgoing: outgoing != 0,
                kind,
                created_at,
            })
        })?;
        let mut list: Vec<ChatMessage> = rows.filter_map(|r| r.ok()).collect();
        list.reverse();
        Ok(list)
    }

    /// 清空与某个 peer 的聊天记录（会话内"清空聊天"按钮）
    pub fn clear_history(&self, peer_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM messages WHERE peer_id=?1", rusqlite::params![peer_id])
            .context("清空聊天记录失败")?;
        Ok(())
    }

    /// 最近有会话的 peer 列表（peer_id, last_time）——设备离线也显示
    pub fn recent_peers(&self, limit: i64) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT peer_id, MAX(created_at) AS t FROM messages GROUP BY peer_id ORDER BY t DESC LIMIT ?1")
            .context("查询最近会话失败")?;
        let rows = stmt.query_map(rusqlite::params![limit], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 记住设备名（DeviceUp 时调用），离线后侧栏仍能显示名字
    pub fn upsert_peer(&self, id: &str, name: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO peers(id, name, last_seen) VALUES(?1,?2,?3)
                 ON CONFLICT(id) DO UPDATE SET name=?2, last_seen=?3",
                rusqlite::params![id, name, crate::proto::now_ms()],
            )
            .context("更新设备名失败")?;
        Ok(())
    }

    pub fn known_peer_names(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM peers")
            .context("查询设备名失败")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn peer_display_name(&self, peer_id: &str) -> Result<Option<String>> {
        // 从最近一条消息上下文里找名字不可靠；名字由发现协议维护，此处仅返回 None 交给 UI 用 id
        let _ = peer_id;
        Ok(None)
    }
}

/// 参数校验辅助：ensure 目录存在
pub fn ensure_dir(p: &Path) -> Result<()> {
    if !p.exists() {
        std::fs::create_dir_all(p).with_context(|| format!("创建目录 {} 失败", p.display()))?;
    } else if !p.is_dir() {
        bail!("{} 不是目录", p.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_store() -> Store {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        Store { conn }
    }

    #[test]
    fn text_roundtrip() {
        let s = mem_store();
        let id = s.insert_text("peer1", true, "hello", 1000).unwrap();
        assert_eq!(id, 1);
        let id2 = s.insert_text("peer1", false, "world", 1001).unwrap();
        assert_eq!(id2, 2);
        let h = s.load_history("peer1", 10).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].kind, MessageKind::Text("hello".into()));
        assert!(h[0].outgoing);
        assert!(!h[1].outgoing);
        let peers = s.recent_peers(10).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].0, "peer1");
    }

    #[test]
    fn file_roundtrip() {
        let s = mem_store();
        let id = s
            .insert_file("p", false, "a.txt", 12, None, Some("t1"), Some("f1"), 1)
            .unwrap();
        s.update_file_path(id, Path::new("D:/x/a.txt")).unwrap();
        let h = s.load_history("p", 10).unwrap();
        match &h[0].kind {
            MessageKind::File {
                name,
                size,
                saved_path,
                transfer_id,
                file_id,
            } => {
                assert_eq!(name, "a.txt");
                assert_eq!(*size, 12);
                assert_eq!(saved_path.as_deref(), Some(Path::new("D:/x/a.txt")));
                assert_eq!(transfer_id.as_deref(), Some("t1"));
                assert_eq!(file_id.as_deref(), Some("f1"));
            }
            other => panic!("kind 不对: {:?}", other),
        }
    }
}
